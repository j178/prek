use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use futures::TryFutureExt;
use prek_consts::env_vars::EnvVars;
use prek_consts::prepend_paths;
use tokio::fs;
use tracing::debug;

use crate::cli::reporter::{HookInstallReporter, HookRunReporter};
use crate::hook::{Hook, InstallInfo, InstalledHook};
use crate::languages::LanguageImpl;
use crate::languages::dotnet::installer::{installer_from_store, query_dotnet_version};
use crate::languages::version::LanguageRequest;
use crate::process::Cmd;
use crate::run::run_by_batch;
use crate::store::Store;

#[derive(Debug, Copy, Clone)]
pub(crate) struct Dotnet;

fn tools_path(env_path: &Path) -> PathBuf {
    env_path.join("tools")
}

impl LanguageImpl for Dotnet {
    async fn install(
        &self,
        hook: Arc<Hook>,
        store: &Store,
        reporter: &HookInstallReporter,
    ) -> Result<InstalledHook> {
        let progress = reporter.on_install_start(&hook);

        let mut info = InstallInfo::new(
            hook.language,
            hook.env_key_dependencies().clone(),
            &store.hooks_dir(),
        )?;

        debug!(%hook, target = %info.env_path.display(), "Installing dotnet environment");

        // Install or find dotnet SDK
        let allows_download = !matches!(
            hook.language_request,
            LanguageRequest::Any { system_only: true }
        );
        let installer = installer_from_store(store);
        let dotnet_result = installer
            .install(&hook.language_request, allows_download)
            .await
            .context("Failed to install or find dotnet SDK")?;

        let tool_path = tools_path(&info.env_path);
        if !hook.additional_dependencies.is_empty() {
            fs_err::tokio::create_dir_all(&tool_path).await?;
            for dep in &hook.additional_dependencies {
                install_tool(dotnet_result.dotnet(), &tool_path, dep).await?;
            }
        }

        info.with_language_version(dotnet_result.version().clone())
            .with_toolchain(dotnet_result.dotnet().to_path_buf());
        info.persist_env_path();

        reporter.on_install_complete(progress);

        Ok(InstalledHook::Installed {
            hook,
            info: Arc::new(info),
        })
    }

    async fn check_health(&self, info: &InstallInfo) -> Result<()> {
        let current_version = query_dotnet_version(&info.toolchain)
            .await
            .context("Failed to query current dotnet info")?;

        // Only check major.minor for compatibility
        if current_version.major != info.language_version.major
            || current_version.minor != info.language_version.minor
        {
            anyhow::bail!(
                "dotnet version mismatch: expected `{}.{}`, found `{}.{}`",
                info.language_version.major,
                info.language_version.minor,
                current_version.major,
                current_version.minor
            );
        }

        Ok(())
    }

    async fn run(
        &self,
        hook: &InstalledHook,
        filenames: &[&Path],
        _store: &Store,
        reporter: &HookRunReporter,
    ) -> Result<(i32, Vec<u8>)> {
        let progress = reporter.on_run_start(hook, filenames.len());

        let env_dir = hook.env_path().expect("Dotnet must have env path");
        let tool_path = tools_path(env_dir);
        let toolchain_path = hook
            .install_info()
            .expect("Dotnet must have install info")
            .toolchain
            .clone();

        // Resolve any symlinks in the dotnet executable path and use its parent
        // directory as both the PATH entry and DOTNET_ROOT. This avoids setting
        // DOTNET_ROOT to a shim directory such as /usr/bin.
        let canonical_path = fs::canonicalize(&toolchain_path)
            .await
            .context("Failed to resolve dotnet toolchain path")?;

        let dotnet_root = canonical_path
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| anyhow::anyhow!("Canonicalized dotnet executable must have parent"))?;

        let new_path = prepend_paths(&[&tool_path, &dotnet_root]).context("Failed to join PATH")?;
        let entry = hook.entry.resolve(Some(&new_path))?;

        let run = async |batch: &[&Path]| {
            let mut output = Cmd::new(&entry[0], "run dotnet hook")
                .current_dir(hook.work_dir())
                .args(&entry[1..])
                .env(EnvVars::PATH, &new_path)
                .env(EnvVars::DOTNET_ROOT, &dotnet_root)
                .envs(&hook.env)
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

        let results = run_by_batch(hook, filenames, &entry, run).await?;

        reporter.on_run_complete(progress);

        let mut combined_status = 0;
        let mut combined_output = Vec::new();

        for (code, output) in results {
            combined_status |= code;
            combined_output.extend(output);
        }

        Ok((combined_status, combined_output))
    }
}

/// Install a dotnet tool as an additional dependency.
///
/// The dependency can be specified as:
/// - `package` - installs latest version
/// - `package:version` - installs specific version
async fn install_tool(dotnet: &Path, tool_path: &Path, dependency: &str) -> Result<()> {
    let (package, version) = dependency
        .split_once(':')
        .map_or((dependency, None), |(pkg, ver)| (pkg, Some(ver)));

    let mut cmd = Cmd::new(dotnet, "dotnet tool install");
    cmd.arg("tool")
        .arg("install")
        .arg("--tool-path")
        .arg(tool_path)
        .arg(package);

    if let Some(ver) = version {
        cmd.arg("--version").arg(ver);
    }

    cmd.check(true)
        .output()
        .await
        .with_context(|| format!("Failed to install dotnet tool: {dependency}"))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use rustc_hash::FxHashSet;

    use crate::config::Language;
    use crate::hook::InstallInfo;
    use crate::languages::LanguageImpl;
    use crate::languages::dotnet::installer::query_dotnet_version;

    use super::Dotnet;

    fn dotnet_path() -> std::path::PathBuf {
        which::which("dotnet").expect("dotnet must be installed to run this test")
    }

    #[tokio::test]
    async fn test_check_health() -> anyhow::Result<()> {
        let dotnet_path = dotnet_path();
        let version = query_dotnet_version(&dotnet_path).await?;

        let temp_dir = tempfile::tempdir()?;
        let mut install_info =
            InstallInfo::new(Language::Dotnet, FxHashSet::default(), temp_dir.path())?;
        install_info
            .with_language_version(version)
            .with_toolchain(dotnet_path);

        // Test the Dotnet impl directly
        let result = Dotnet.check_health(&install_info).await;
        assert!(result.is_ok());

        // Also test through Language dispatch
        let result = Language::Dotnet.check_health(&install_info).await;
        assert!(result.is_ok());

        Ok(())
    }

    #[tokio::test]
    async fn test_check_health_version_mismatch() -> anyhow::Result<()> {
        let dotnet_path = dotnet_path();

        let temp_dir = tempfile::tempdir()?;
        let mut install_info =
            InstallInfo::new(Language::Dotnet, FxHashSet::default(), temp_dir.path())?;
        // Use a fake version that won't match the actual dotnet version
        install_info
            .with_language_version(semver::Version::new(1, 0, 0))
            .with_toolchain(dotnet_path);

        let result = Dotnet.check_health(&install_info).await;
        assert!(result.is_err());

        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("dotnet version mismatch"),
            "expected version mismatch error, got: {err}"
        );

        Ok(())
    }
}
