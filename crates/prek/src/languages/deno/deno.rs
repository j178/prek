use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;

use anyhow::{Context, Result};
use prek_consts::env_vars::EnvVars;
use prek_consts::prepend_paths;
use tracing::debug;

use crate::cli::reporter::{HookInstallReporter, HookRunReporter};
use crate::hook::{Hook, InstallInfo, InstalledHook};
use crate::languages::LanguageImpl;
use crate::languages::deno::DenoRequest;
use crate::languages::deno::installer::{DenoInstaller, DenoResult, bin_dir};
use crate::languages::version::LanguageRequest;
use crate::process::Cmd;
use crate::run::run_by_batch;
use crate::store::{CacheBucket, Store, ToolBucket};

fn is_valid_install_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphanumeric())
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Parse a Deno `additional_dependencies` item.
///
/// Deno support treats every additional dependency as an executable install target for
/// `deno install --global`. That makes the contract explicit and avoids guessing whether
/// a string should be handled as `deno add` or `deno install`.
///
/// The optional `:name` suffix is interpreted as `deno install --name <name>`, but only
/// when the left side clearly looks like an install target that may legitimately contain
/// colons itself:
/// - specifiers such as `npm:semver@7`
/// - URLs such as `https://...`
/// - local paths such as `./cli.ts`
///
/// Plain command strings are left untouched so we do not accidentally split on a colon
/// that is part of the dependency string.
fn parse_install_dependency(spec: &str) -> (&str, Option<&str>) {
    let Some((dep, name)) = spec.rsplit_once(':') else {
        return (spec, None);
    };

    let looks_like_path = dep.starts_with('.') || dep.starts_with('/') || dep.contains(['/', '\\']);

    if is_valid_install_name(name) && (looks_like_path || dep.contains(':')) {
        (dep, Some(name))
    } else {
        (spec, None)
    }
}

#[derive(Debug, Copy, Clone)]
pub(crate) struct Deno;

impl LanguageImpl for Deno {
    async fn install(
        &self,
        hook: Arc<Hook>,
        store: &Store,
        reporter: &HookInstallReporter,
    ) -> Result<InstalledHook> {
        let progress = reporter.on_install_start(&hook);

        // 1. Install deno
        let deno_dir = store.tools_path(ToolBucket::Deno);
        let installer = DenoInstaller::new(deno_dir);

        let (deno_request, allows_download) = match &hook.language_request {
            LanguageRequest::Any { system_only } => (&DenoRequest::Any, !system_only),
            LanguageRequest::Deno(deno_request) => (deno_request, true),
            _ => unreachable!(),
        };
        let deno = installer
            .install(store, deno_request, allows_download)
            .await
            .context("Failed to install deno")?;

        let mut info = InstallInfo::new(
            hook.language,
            hook.env_key_dependencies().clone(),
            &store.hooks_dir(),
        )?;

        info.with_toolchain(deno.deno().to_path_buf());
        info.with_language_version((**deno.version()).clone());

        // 2. Create env
        let env_bin_dir = bin_dir(&info.env_path);
        fs_err::tokio::create_dir_all(&env_bin_dir).await?;

        // Relative install targets in `additional_dependencies` are resolved by Deno
        // against the process working directory. For remote hooks that should be the
        // cloned hook repository so `./cli.ts:name` refers to files shipped by the hook.
        // For local hooks we keep resolution in the user's work tree.
        let install_dir = hook.repo_path().unwrap_or(hook.work_dir());

        // We share one Deno cache bucket across install and run. Executable shims live in
        // the per-hook env bin dir, while downloaded modules and npm artifacts are reused
        // from this cache bucket.
        let deno_cache_dir = store.cache_path(CacheBucket::Deno);
        fs_err::tokio::create_dir_all(&deno_cache_dir).await?;

        // 3. Install additional dependencies as executables in the hook env.
        //
        // Current Deno contract:
        // - prek does not try to install the remote hook repo itself
        // - prek does not inspect or rewrite `entry` to derive install targets
        // - every `additional_dependencies` item is provisioned into `<env>/bin`
        //
        // This keeps installation and execution separate. If a remote hook repo wants to
        // expose its own executable, it must declare a local file in
        // `additional_dependencies`, for example `./cli.ts:repo-tool`, and then use
        // `repo-tool` in `entry`.
        //
        // We intentionally pass `--allow-all` because `deno install` bakes permissions into
        // the installed wrapper. Since prek does not parse `entry` or repo metadata to infer
        // a minimal permission set, the simplest predictable behavior is to install the
        // executable with full permissions and let the hook author choose the installed
        // command name explicitly when needed via `dep:name`.
        if !hook.additional_dependencies.is_empty() {
            debug!(deps = ?hook.additional_dependencies, "Installing deno dependencies");
        }
        for spec in &hook.additional_dependencies {
            let (dep, name) = parse_install_dependency(spec);

            let mut install_cmd = Cmd::new(deno.deno(), "deno install dependency");
            install_cmd
                .current_dir(install_dir)
                .env(EnvVars::DENO_DIR, &deno_cache_dir)
                .env(EnvVars::DENO_NO_UPDATE_CHECK, "1")
                .arg("install")
                .arg("--allow-all")
                .arg("--global")
                .arg("--force")
                .arg("--root")
                .arg(&info.env_path);

            if let Some(name) = name {
                install_cmd.arg("--name").arg(name);
            }

            install_cmd
                .arg(dep)
                .check(true)
                .output()
                .await
                .with_context(|| format!("Failed to install deno dependency `{spec}`"))?;
        }

        info.persist_env_path();

        reporter.on_install_complete(progress);

        Ok(InstalledHook::Installed {
            hook,
            info: Arc::new(info),
        })
    }

    async fn check_health(&self, info: &InstallInfo) -> Result<()> {
        let deno = DenoResult::from_executable(info.toolchain.clone())
            .fill_version()
            .await
            .context("Failed to query deno version")?;

        if **deno.version() != info.language_version {
            anyhow::bail!(
                "Deno version mismatch: expected {}, found {}",
                info.language_version,
                deno.version()
            );
        }

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

        let deno_cache_dir = store.cache_path(CacheBucket::Deno);
        let info = hook.install_info().expect("Deno must be installed");
        let env_dir = &info.env_path;
        let deno_bin_dir = hook.toolchain_dir().expect("Deno must have toolchain dir");
        let new_path =
            prepend_paths(&[&bin_dir(env_dir), deno_bin_dir]).context("Failed to join PATH")?;

        let entry = hook.entry.resolve(Some(&new_path), store)?;

        let run = async |batch: &[&Path]| {
            let mut cmd = Cmd::new(&entry[0], "deno hook");
            let mut output = cmd
                .current_dir(hook.work_dir())
                .env(EnvVars::PATH, &new_path)
                .env(EnvVars::DENO_DIR, &deno_cache_dir)
                .env(EnvVars::DENO_NO_UPDATE_CHECK, "1")
                .envs(&hook.env)
                .args(&entry[1..])
                .args(&hook.args)
                .args(batch)
                .check(false)
                .stdin(Stdio::null())
                .pty_output()
                .await?;

            reporter.on_run_progress(progress, batch.len() as u64);

            output.stdout.extend(output.stderr);
            let code = output.status.code().unwrap_or(1);
            anyhow::Ok((code, output.stdout))
        };

        let results = run_by_batch(hook, filenames, entry.argv(), run).await?;

        reporter.on_run_complete(progress);

        // Collect results
        let mut combined_status = 0;
        let mut combined_output = Vec::new();

        for (code, output) in results {
            combined_status |= code;
            combined_output.extend(output);
        }

        Ok((combined_status, combined_output))
    }
}

#[cfg(test)]
mod tests {
    use super::parse_install_dependency;

    #[test]
    fn parse_install_dependency_without_name() {
        assert_eq!(
            parse_install_dependency("npm:prettier@3"),
            ("npm:prettier@3", None)
        );
    }

    #[test]
    fn parse_install_dependency_with_name() {
        assert_eq!(
            parse_install_dependency("npm:prettier@3:fmt-tool"),
            ("npm:prettier@3", Some("fmt-tool"))
        );
    }

    #[test]
    fn parse_install_dependency_with_local_path_name() {
        assert_eq!(
            parse_install_dependency("./tools/echo.ts:echo-tool"),
            ("./tools/echo.ts", Some("echo-tool"))
        );
    }

    #[test]
    fn parse_install_dependency_with_invalid_name_keeps_original() {
        assert_eq!(
            parse_install_dependency("./tools/echo.ts:not valid"),
            ("./tools/echo.ts:not valid", None)
        );
    }
}
