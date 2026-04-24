use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use anyhow::{Context, Result};
use prek_consts::env_vars::EnvVars;
use prek_consts::prepend_paths;
use tracing::debug;

use crate::cli::reporter::{HookInstallReporter, HookRunReporter};
use crate::hook::{Hook, InstallInfo, InstalledHook};
use crate::languages::LanguageImpl;
use crate::languages::dotnet::DotnetRequest;
use crate::languages::dotnet::installer::{DotnetInstaller, DotnetResult};
use crate::languages::version::LanguageRequest;
use crate::process::Cmd;
use crate::run::run_by_batch;
use crate::store::{Store, ToolBucket};

#[derive(Debug, Copy, Clone)]
pub(crate) struct Dotnet;

fn tools_dir(env_path: &Path) -> PathBuf {
    env_path.join("tools")
}

/// Resolves the runtime root for a dotnet executable.
///
/// For system installs, `toolchain` can point at a shim or symlinked path such
/// as `/usr/bin/dotnet`. Canonicalizing first ensures `DOTNET_ROOT` points at
/// the real SDK/runtime directory.
fn resolve_dotnet_root(dotnet: &Path) -> Result<PathBuf> {
    let dotnet = fs_err::canonicalize(dotnet).unwrap_or_else(|_| dotnet.to_path_buf());
    dotnet
        .parent()
        .map(Path::to_path_buf)
        .context("dotnet executable must have parent")
}

impl LanguageImpl for Dotnet {
    async fn install(
        &self,
        hook: Arc<Hook>,
        store: &Store,
        reporter: &HookInstallReporter,
    ) -> Result<InstalledHook> {
        let progress = reporter.on_install_start(&hook);

        let installer = DotnetInstaller::new(store.tools_path(ToolBucket::Dotnet));
        let (request, allows_download) = match &hook.language_request {
            LanguageRequest::Any { system_only } => (&DotnetRequest::Any, !system_only),
            LanguageRequest::Dotnet(request) => (request, true),
            _ => unreachable!(),
        };
        let dotnet = installer
            .install(request, allows_download)
            .await
            .context("Failed to install dotnet SDK")?;

        let mut info = InstallInfo::new(
            hook.language,
            hook.env_key_dependencies().clone(),
            &store.hooks_dir(),
        )?;

        let tools_dir = tools_dir(&info.env_path);

        debug!(
            path = %tools_dir.display(),
            "Installing additional dotnet tools for hook"
        );
        if !hook.additional_dependencies.is_empty() {
            fs_err::tokio::create_dir_all(&tools_dir).await?;
            for dependency in &hook.additional_dependencies {
                install_tool(dotnet.dotnet(), &tools_dir, dependency).await?;
            }
        }

        info.with_language_version((**dotnet.version()).clone())
            .with_toolchain(dotnet.dotnet().to_path_buf());

        info.persist_env_path();
        reporter.on_install_complete(progress);

        Ok(InstalledHook::Installed {
            hook,
            info: Arc::new(info),
        })
    }

    async fn check_health(&self, info: &InstallInfo) -> Result<()> {
        let current_version = DotnetResult::from_executable(info.toolchain.clone())
            .fill_version()
            .await
            .context("Failed to query current dotnet info")?;

        // Only check major.minor for compatibility
        if current_version.version().major != info.language_version.major
            || current_version.version().minor != info.language_version.minor
        {
            anyhow::bail!(
                "dotnet version mismatch: expected `{}.{}`, found `{}.{}`",
                info.language_version.major,
                info.language_version.minor,
                current_version.version().major,
                current_version.version().minor
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

        let env_dir = hook.env_path().expect("dotnet hook must have env path");
        let tools_dir = tools_dir(env_dir);
        let dotnet = &hook
            .install_info()
            .expect("dotnet hook must have install info")
            .toolchain;
        let dotnet_root = resolve_dotnet_root(dotnet).context("Failed to resolve DOTNET_ROOT")?;

        let new_path = prepend_paths(&[&tools_dir, &dotnet_root]).context("Failed to join PATH")?;
        let entry = hook.entry.resolve(Some(&new_path), store)?;

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

        let results = run_by_batch(hook, filenames, entry.argv(), run).await?;

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
async fn install_tool(dotnet: &Path, tool_dir: &Path, dependency: &str) -> Result<()> {
    let (package, version) = dependency
        .split_once(':')
        .map_or((dependency, None), |(package, version)| {
            (package, Some(version))
        });

    let tool_cmd = |action: &str| {
        let mut cmd = Cmd::new(dotnet, format!("dotnet tool {action}"));
        cmd.arg("tool")
            .arg(action)
            .arg("--tool-path")
            .arg(tool_dir)
            .arg(package);
        if let Some(version) = version {
            cmd.arg("--version").arg(version);
        }
        cmd
    };

    match tool_cmd("install").check(true).output().await {
        Ok(_) => Ok(()),
        Err(err) => {
            if err.to_string().contains("is already installed") {
                debug!(
                    package,
                    path = %tool_dir.display(),
                    "Dotnet tool already installed, attempting update"
                );
                tool_cmd("update")
                    .check(true)
                    .output()
                    .await
                    .with_context(|| format!("Failed to update dotnet tool: {dependency}"))?;
                Ok(())
            } else {
                Err(err).with_context(|| format!("Failed to install dotnet tool {dependency}"))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::resolve_dotnet_root;
    #[test]
    fn resolve_dotnet_root_falls_back_to_declared_parent_when_missing() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let dotnet = temp_dir.path().join("missing").join("dotnet");

        assert_eq!(
            resolve_dotnet_root(&dotnet)?,
            temp_dir.path().join("missing")
        );

        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn resolve_dotnet_root_uses_canonicalized_parent_for_symlink() -> anyhow::Result<()> {
        use assert_fs::fixture::{PathChild, PathCreateDir};
        use std::os::unix::fs::symlink;

        let temp_dir = assert_fs::TempDir::new()?;
        let real_root = temp_dir.child("real");
        let shim_root = temp_dir.child("shim");
        real_root.create_dir_all()?;
        shim_root.create_dir_all()?;

        let real_dotnet = real_root.child("dotnet");
        fs_err::write(real_dotnet.path(), "#!/bin/sh\n")?;

        let shim_dotnet = shim_root.child("dotnet");
        symlink(real_dotnet.path(), shim_dotnet.path())?;

        assert_eq!(
            resolve_dotnet_root(shim_dotnet.path())?,
            real_root.path().canonicalize()?
        );

        temp_dir.close()?;
        Ok(())
    }
}
