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
use crate::languages::LanguageBackend;
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

#[async_trait::async_trait(?Send)]
impl LanguageBackend for Coursier {
    async fn install(
        &self,
        store: &Store,
        hook: Arc<Hook>,
        reporter: &HookInstallReporter,
    ) -> Result<InstalledHook> {
        let progress = reporter.on_install_start(&hook);

        let dependencies = &hook.additional_dependencies;

        let cs = which::which("cs")
            .or_else(|_| which::which("coursier"))
            .context(
                "Coursier hooks require system-installed `cs` or `coursier` executables in PATH",
            )?;
        let mut info = InstallInfo::new(&hook, &store.hooks_dir())?;

        debug!(%hook, target = %info.env_path.display(), "Installing Coursier environment");

        fs_err::tokio::create_dir_all(&info.env_path).await?;
        let coursier_cache = store.cache_path(CacheBucket::Coursier);
        fs_err::tokio::create_dir_all(&coursier_cache).await?;

        let path_env = prepend_paths(&[&info.env_path]).context("Failed to join PATH")?;
        let mut has_channel_apps = false;

        if let Some(repo_path) = hook.repo_path() {
            let channel_dir = repo_path.join(PRE_COMMIT_CHANNEL_DIR);
            if let Some(channel_apps) = collect_channel_apps(&channel_dir)? {
                has_channel_apps = true;
                for app in channel_apps {
                    Cmd::new(&cs)
                        .current_dir(repo_path)
                        .arg("install")
                        .arg("--dir")
                        .arg(&info.env_path)
                        .arg("--default-channels=false")
                        .arg("--channel")
                        .arg(&channel_dir)
                        .arg(&app)
                        .env(EnvVars::PATH, &path_env)
                        .env(EnvVars::COURSIER_CACHE, &coursier_cache)
                        .check(true)
                        .output()
                        .await
                        .with_context(|| format!("Failed to install coursier app `{app}`"))?;
                }
            }
        }

        if !dependencies.is_empty() {
            let mut fetch_cmd = Cmd::new(&cs);
            fetch_cmd
                .arg("fetch")
                .args(dependencies)
                .env(EnvVars::PATH, &path_env)
                .env(EnvVars::COURSIER_CACHE, &coursier_cache);
            if let Some(repo_path) = hook.repo_path() {
                fetch_cmd.current_dir(repo_path);
            }
            fetch_cmd.check(true).output().await.with_context(|| {
                format!("Failed to fetch coursier app `{}`", dependencies.join(" "))
            })?;

            let mut install_cmd = Cmd::new(&cs);
            install_cmd
                .arg("install")
                .arg("--dir")
                .arg(&info.env_path)
                .args(dependencies)
                .env(EnvVars::PATH, path_env)
                .env(EnvVars::COURSIER_CACHE, &coursier_cache);
            if let Some(repo_path) = hook.repo_path() {
                install_cmd.current_dir(repo_path);
            }
            install_cmd.check(true).output().await.with_context(|| {
                format!(
                    "Failed to install coursier app `{}`",
                    dependencies.join(" ")
                )
            })?;
        } else if !has_channel_apps {
            anyhow::bail!("expected `.pre-commit-channel` directory or `additional_dependencies`");
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
        store: &Store,
        hook: &InstalledHook,
        filenames: &[&Path],
        reporter: &HookRunReporter,
    ) -> Result<(i32, Vec<u8>)> {
        let progress = reporter.on_run_start(hook, filenames.len());

        let env_path = hook.env_path().expect("Coursier must have env path");
        let coursier_cache = store.cache_path(CacheBucket::Coursier);
        let path_env = prepend_paths(&[env_path]).context("Failed to join PATH")?;
        let entry = hook.entry.resolve(Some(&path_env), store)?;

        let run = async |batch: &[&Path]| {
            let mut output = Cmd::new(&entry[0])
                .current_dir(hook.work_dir())
                .args(&entry[1..])
                .envs(&hook.env)
                .args(&hook.args)
                .file_args(batch)
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
