use std::ffi::OsStr;
use std::io::ErrorKind;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;

use anyhow::{Context, Result};
use prek_consts::env_vars::EnvVars;
use prek_consts::prepend_paths;
use tracing::debug;

use crate::cli::reporter::HookInstallReporter;
use crate::cli::run::HookRunReporter;
use crate::hook::{Hook, InstallInfo, InstalledHook};
use crate::languages::LanguageImpl;
use crate::process::Cmd;
use crate::run::run_by_batch;
use crate::store::{CacheBucket, Store};

const PRE_COMMIT_CHANNEL_DIR: &str = ".pre-commit-channel";

#[derive(Debug, Copy, Clone)]
pub(crate) struct Coursier;

fn channel_app_name(file_name: &str) -> &str {
    match file_name.rfind('.') {
        Some(0) | None => file_name,
        Some(index) if index + 1 == file_name.len() => file_name,
        Some(index) => &file_name[..index],
    }
}

fn collect_channel_apps(channel_dir: &Path) -> Result<Option<Vec<String>>> {
    let entries = match fs_err::read_dir(channel_dir) {
        Ok(entries) => entries,
        Err(err) if matches!(err.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) => {
            return Ok(None);
        }
        Err(err) => {
            return Err(err).with_context(|| format!("Failed to read `{}`", channel_dir.display()));
        }
    };

    let mut apps = entries
        .map(|entry| {
            let file_name = entry?.file_name();
            let file_name = file_name.to_string_lossy();
            Ok(channel_app_name(&file_name).to_string())
        })
        .collect::<Result<Vec<_>>>()?;
    apps.sort_unstable();
    Ok(Some(apps))
}

async fn install_coursier_app(
    cs: &Path,
    env_path: &Path,
    cache_path: &Path,
    source_path: &Path,
    path_env: &OsStr,
    args: &[String],
) -> Result<()> {
    Cmd::new(cs, "coursier install")
        .current_dir(source_path)
        .arg("install")
        .arg("--dir")
        .arg(env_path)
        .args(args)
        .env(EnvVars::PATH, path_env)
        .env(EnvVars::COURSIER_CACHE, cache_path)
        .check(true)
        .output()
        .await
        .with_context(|| format!("Failed to install coursier app `{}`", args.join(" ")))?;

    Ok(())
}

impl LanguageImpl for Coursier {
    async fn install(
        &self,
        hook: Arc<Hook>,
        store: &Store,
        reporter: &HookInstallReporter,
    ) -> Result<InstalledHook> {
        let progress = reporter.on_install_start(&hook);

        let source_path = hook.repo_path().unwrap_or_else(|| hook.work_dir());
        let channel_dir = source_path.join(PRE_COMMIT_CHANNEL_DIR);
        let channel_apps = collect_channel_apps(&channel_dir)?;

        let mut dependencies = hook
            .additional_dependencies
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        dependencies.sort_unstable();

        if channel_apps.is_none() && dependencies.is_empty() {
            anyhow::bail!("expected .pre-commit-channel dir or additional_dependencies");
        }

        let cs = which::which("cs")
            .or_else(|_| which::which("coursier"))
            .context(
                "Coursier hooks require system-installed `cs` or `coursier` executables in PATH",
            )?;
        let mut info = InstallInfo::new(
            hook.language,
            hook.env_key_dependencies().clone(),
            &store.hooks_dir(),
        )?;

        debug!(%hook, target = %info.env_path.display(), "Installing Coursier environment");

        fs_err::tokio::create_dir_all(&info.env_path).await?;
        let coursier_cache = store.cache_path(CacheBucket::Coursier);
        fs_err::tokio::create_dir_all(&coursier_cache).await?;

        let path_env = prepend_paths(&[&info.env_path]).context("Failed to join PATH")?;

        if let Some(channel_apps) = channel_apps {
            for app in channel_apps {
                let args = vec![
                    "--default-channels=false".to_string(),
                    "--channel".to_string(),
                    channel_dir.to_string_lossy().into_owned(),
                    app,
                ];
                install_coursier_app(
                    &cs,
                    &info.env_path,
                    &coursier_cache,
                    source_path,
                    &path_env,
                    &args,
                )
                .await?;
            }
        }

        if !dependencies.is_empty() {
            Cmd::new(&cs, "coursier fetch")
                .current_dir(source_path)
                .arg("fetch")
                .args(&dependencies)
                .env(EnvVars::PATH, &path_env)
                .env(EnvVars::COURSIER_CACHE, &coursier_cache)
                .check(true)
                .output()
                .await
                .with_context(|| {
                    format!("Failed to fetch coursier app `{}`", dependencies.join(" "))
                })?;
            install_coursier_app(
                &cs,
                &info.env_path,
                &coursier_cache,
                source_path,
                &path_env,
                &dependencies,
            )
            .await?;
        }

        info.with_toolchain(cs);
        info.persist_env_path();

        reporter.on_install_complete(progress);

        Ok(InstalledHook::Installed {
            hook,
            info: Arc::new(info),
        })
    }

    async fn check_health(&self, _info: &InstallInfo) -> Result<()> {
        Ok(())
    }

    async fn run(
        &self,
        hook: &InstalledHook,
        filenames: &[&Path],
        store: &Store,
        reporter: &HookRunReporter,
    ) -> Result<(i32, Vec<u8>)> {
        let progress = reporter.on_run_start(hook, filenames.len());

        let env_path = hook.env_path().expect("Coursier must have env path");
        let coursier_cache = store.cache_path(CacheBucket::Coursier);
        let path_env = prepend_paths(&[env_path]).context("Failed to join PATH")?;
        let entry = hook.entry.resolve(Some(&path_env), store)?;

        let run = async |batch: &[&Path]| {
            let mut output = Cmd::new(&entry[0], "run coursier hook")
                .current_dir(hook.work_dir())
                .args(&entry[1..])
                .envs(&hook.env)
                .args(&hook.args)
                .args(batch)
                .check(false)
                .stdin(Stdio::null())
                .env(EnvVars::PATH, &path_env)
                .env(EnvVars::COURSIER_CACHE, &coursier_cache)
                .pty_output_with_sink(reporter.output_sink(progress))
                .await?;

            reporter.on_run_progress(progress, batch.len() as u64);

            output.stdout.extend(output.stderr);
            let code = output.status.code().unwrap_or(1);
            anyhow::Ok((code, output.stdout))
        };

        let results = run_by_batch(hook, filenames, entry.argv(), run).await?;

        let mut combined_status = 0;
        let mut combined_output = Vec::new();

        for (code, output) in results {
            combined_status |= code;
            combined_output.extend(output);
        }

        reporter.on_run_complete(progress);

        Ok((combined_status, combined_output))
    }
}

#[cfg(test)]
mod tests {
    use super::channel_app_name;

    #[test]
    fn channel_app_name_drops_descriptor_extension() {
        assert_eq!(channel_app_name("scalafmt.json"), "scalafmt");
        assert_eq!(channel_app_name("foo.bar.json"), "foo.bar");
    }

    #[test]
    fn channel_app_name_keeps_dotfiles_and_trailing_dots() {
        assert_eq!(channel_app_name(".scalafmt"), ".scalafmt");
        assert_eq!(channel_app_name("scalafmt."), "scalafmt.");
    }
}
