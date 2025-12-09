use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use anyhow::Context;

use prek_consts::env_vars::EnvVars;

use crate::cli::reporter::HookInstallReporter;
use crate::hook::{Hook, InstallInfo, InstalledHook};
use crate::languages::LanguageImpl;
use crate::languages::golang::GoRequest;
use crate::languages::golang::installer::GoInstaller;
use crate::languages::version::LanguageRequest;
use crate::process::Cmd;
use crate::run::{prepend_paths, run_by_batch};
use crate::store::{CacheBucket, Store, ToolBucket};

#[derive(Debug, Copy, Clone)]
pub(crate) struct Golang;

impl LanguageImpl for Golang {
    async fn install(
        &self,
        hook: Arc<Hook>,
        store: &Store,
        reporter: &HookInstallReporter,
    ) -> anyhow::Result<InstalledHook> {
        let progress = reporter.on_install_start(&hook);

        // 1. Install Go
        let go_dir = store.tools_path(ToolBucket::Go);
        let installer = GoInstaller::new(go_dir);

        let (version, allows_download) = match &hook.language_request {
            LanguageRequest::Any { system_only } => (&GoRequest::Any, !system_only),
            LanguageRequest::Golang(version) => (version, true),
            _ => unreachable!(),
        };
        let go = installer
            .install(store, version, allows_download)
            .await
            .context("Failed to install go")?;

        let mut info = InstallInfo::new(
            hook.language,
            hook.dependencies().clone(),
            &store.hooks_dir(),
        )?;
        info.with_toolchain(go.bin().to_path_buf())
            .with_language_version(go.version().deref().clone());

        // 2. Create environment
        fs_err::tokio::create_dir_all(bin_dir(&info.env_path)).await?;

        // 3. Install dependencies
        // go: ~/.cache/prek/tools/go/1.24.0/bin/go
        // go_root: ~/.cache/prek/tools/go/1.24.0
        // go_cache: ~/.cache/prek/cache/go
        // go_bin: ~/.cache/prek/hooks/envs/<hook_id>/bin
        let go_root = go
            .bin()
            .parent()
            .and_then(|p| p.parent())
            .expect("Go root should exist");
        let go_cache = store.cache_path(CacheBucket::Go);

        let go_install_cmd = || {
            if go.is_from_system() {
                let mut cmd = go.cmd("go install");
                cmd.arg("install")
                    .env(EnvVars::GOTOOLCHAIN, "local")
                    .env(EnvVars::GOBIN, bin_dir(&info.env_path));
                cmd
            } else {
                let mut cmd = go.cmd("go install");
                cmd.arg("install")
                    .env(EnvVars::GOTOOLCHAIN, "local")
                    .env(EnvVars::GOROOT, go_root)
                    .env(EnvVars::GOBIN, bin_dir(&info.env_path))
                    .env(EnvVars::GOFLAGS, "-modcacherw")
                    .env(EnvVars::GOPATH, &go_cache);
                cmd
            }
        };

        // GOPATH used to store downloaded source code (in $GOPATH/pkg/mod)
        if let Some(repo) = hook.repo_path() {
            go_install_cmd()
                .arg("./...")
                .current_dir(repo)
                .remove_git_env()
                .check(true)
                .output()
                .await?;
        }
        for dep in &hook.additional_dependencies {
            let mut cmd = go_install_cmd();
            if let Some(repo) = hook.repo_path() {
                cmd.current_dir(repo);
            }
            cmd.arg(dep).remove_git_env().check(true).output().await?;
        }

        info.persist_env_path();

        reporter.on_install_complete(progress);

        Ok(InstalledHook::Installed {
            hook,
            info: Arc::new(info),
        })
    }

    async fn check_health(&self, _info: &InstallInfo) -> anyhow::Result<()> {
        Ok(())
    }

    async fn run(
        &self,
        hook: &InstalledHook,
        filenames: &[&Path],
        store: &Store,
    ) -> anyhow::Result<(i32, Vec<u8>)> {
        let env_dir = hook.env_path().expect("Node hook must have env path");
        let info = hook.install_info().expect("Node hook must be installed");

        let go_bin = bin_dir(env_dir);
        let go_tools = store.tools_path(ToolBucket::Go);
        let go_root_bin = info.toolchain.parent().expect("Go root should exist");
        let go_root = go_root_bin.parent().expect("Go root should exist");
        let go_cache = store.cache_path(CacheBucket::Go);

        // Only set GOROOT and GOPATH if using the Go installed by prek
        let go_envs = if go_root_bin.starts_with(go_tools) {
            vec![(EnvVars::GOROOT, go_root), (EnvVars::GOPATH, &go_cache)]
        } else {
            vec![]
        };
        let new_path = prepend_paths(&[&go_bin, go_root_bin]).context("Failed to join PATH")?;

        let entry = hook.entry.resolve(Some(&new_path))?;
        let run = async |batch: &[&Path]| {
            let mut output = Cmd::new(&entry[0], "go hook")
                .current_dir(hook.work_dir())
                .args(&entry[1..])
                .env(EnvVars::PATH, &new_path)
                .env(EnvVars::GOTOOLCHAIN, "local")
                .env(EnvVars::GOBIN, &go_bin)
                .env(EnvVars::GOFLAGS, "-modcacherw")
                .envs(go_envs.iter().copied())
                .args(&hook.args)
                .args(batch)
                .check(false)
                .stdin(Stdio::null())
                .pty_output()
                .await?;

            output.stdout.extend(output.stderr);
            let code = output.status.code().unwrap_or(1);
            anyhow::Ok((code, output.stdout))
        };

        let results = run_by_batch(hook, filenames, &entry, run).await?;

        let mut combined_status = 0;
        let mut combined_output = Vec::new();

        for (code, output) in results {
            combined_status |= code;
            combined_output.extend(output);
        }

        Ok((combined_status, combined_output))
    }
}

pub(crate) fn bin_dir(env_path: &Path) -> PathBuf {
    env_path.join("bin")
}
