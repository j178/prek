use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context;

use constants::env_vars::EnvVars;

use crate::hook::{Hook, InstallInfo, InstalledHook};
use crate::languages::LanguageImpl;
use crate::languages::golang::GoRequest;
use crate::languages::golang::installer::GoInstaller;
use crate::languages::version::LanguageRequest;
use crate::process::Cmd;
use crate::run::{prepend_paths, run_by_batch};
use crate::store::{CacheBucket, Store};

#[derive(Debug, Copy, Clone)]
pub(crate) struct Golang;

impl LanguageImpl for Golang {
    async fn install(&self, hook: Arc<Hook>, store: &Store) -> anyhow::Result<InstalledHook> {
        // 1. Install Go
        let go_dir = store.tools_path(crate::store::ToolBucket::Go);
        let installer = GoInstaller::new(go_dir);

        let version = match &hook.language_request {
            LanguageRequest::Any => &GoRequest::Any,
            LanguageRequest::Golang(version) => version,
            _ => unreachable!(),
        };
        let go = installer.install(version).await?;

        let mut info = InstallInfo::new(
            hook.language,
            hook.dependencies().clone(),
            &store.hooks_dir(),
        );
        info.with_toolchain(go.bin().to_path_buf())
            .with_language_version(go.version().deref().clone());

        // 2. Create environment
        fs_err::tokio::create_dir_all(&info.env_path).await?;
        fs_err::tokio::create_dir_all(bin_dir(&info.env_path)).await?;

        // 3. Install dependencies
        // go: ~/.cache/prefligit/tools/go/1.24.0/bin/go
        // go_root: ~/.cache/prefligit/tools/go/1.24.0
        // go_cache: ~/.cache/prefligit/cache/go
        // go_bin: ~/.cache/prefligit/hooks/envs/<hook_id>/bin
        let go_root = go
            .bin()
            .parent()
            .and_then(|p| p.parent())
            .expect("Go root should exist");
        let go_cache = store.cache_path(CacheBucket::Go);
        // GOPATH used to store downloaded source code (in $GOPATH/pkg/mod)
        if let Some(repo) = hook.repo_path() {
            go.cmd("go install")
                .arg("install")
                .arg("./...")
                .env(EnvVars::GOTOOLCHAIN, "local")
                .env(EnvVars::GOROOT, go_root)
                .env(EnvVars::GOBIN, bin_dir(&info.env_path))
                .env(EnvVars::GOPATH, &go_cache)
                .current_dir(repo)
                .check(true)
                .output()
                .await?;
        }
        for dep in &hook.additional_dependencies {
            go.cmd("go install")
                .arg("install")
                .arg(dep)
                .env(EnvVars::GOTOOLCHAIN, "local")
                .env(EnvVars::GOROOT, go_root)
                .env(EnvVars::GOBIN, bin_dir(&info.env_path))
                .env(EnvVars::GOPATH, &go_cache)
                .check(true)
                .output()
                .await?;
        }

        Ok(InstalledHook::Installed {
            hook,
            info: Arc::new(info),
        })
    }

    async fn check_health(&self) -> anyhow::Result<()> {
        todo!()
    }

    async fn run(
        &self,
        hook: &InstalledHook,
        filenames: &[&String],
        store: &Store,
    ) -> anyhow::Result<(i32, Vec<u8>)> {
        let env_dir = hook.env_path().expect("Node must have env path");
        let InstalledHook::Installed { hook, info } = hook else {
            unreachable!()
        };

        let go_cache = store.cache_path(CacheBucket::Go);
        let go_root_bin = info.toolchain.parent().expect("Go root should exist");
        let go_root = go_root_bin.parent().expect("Go root should exist");
        let go_bin = bin_dir(env_dir);
        let new_path = prepend_paths(&[&go_bin, go_root_bin]).context("Failed to join PATH")?;

        let entry = hook.entry.parsed()?;
        let run = async move |batch: Vec<String>| {
            let mut output = Cmd::new(&entry[0], "go hook")
                .args(&entry[1..])
                .env("PATH", &new_path)
                .env(EnvVars::GOTOOLCHAIN, "local")
                .env(EnvVars::GOROOT, go_root)
                .env(EnvVars::GOBIN, &go_bin)
                .env(EnvVars::GOPATH, &go_cache)
                .args(&hook.args)
                .args(batch)
                .check(false)
                .output()
                .await?;

            output.stdout.extend(output.stderr);
            let code = output.status.code().unwrap_or(1);
            anyhow::Ok((code, output.stdout))
        };

        let results = run_by_batch(hook, filenames, run).await?;

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
