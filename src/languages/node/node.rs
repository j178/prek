use std::borrow::Cow;
use std::env::consts::EXE_EXTENSION;
use std::sync::Arc;

use anyhow::{Context, Result};
use rustc_hash::FxHashSet;
use tracing::debug;

use constants::env_vars::EnvVars;

use crate::hook::InstalledHook;
use crate::hook::{Hook, InstallInfo};
use crate::languages::node::NodeRequest;
use crate::languages::node::installer::{NodeInstaller, bin_dir, lib_dir};
use crate::languages::node::version::EXTRA_KEY_LTS;
use crate::languages::version::LanguageRequest;
use crate::languages::{LanguageImpl, create_symlink_or_copy};
use crate::process::Cmd;
use crate::run::{prepend_paths, run_by_batch};
use crate::store::{Store, ToolBucket};

#[derive(Debug, Copy, Clone)]
pub(crate) struct Node;

impl LanguageImpl for Node {
    async fn install(&self, hook: Arc<Hook>, store: &Store) -> Result<InstalledHook> {
        // 1. Install node
        //   1) Find from `$PREFLIGIT_HOME/tools/node`
        //   2) Find from system
        //   3) Download from remote
        // 2. Create env
        // 3. Install dependencies

        // 1. Install node
        let node_dir = store.tools_path(ToolBucket::Node);
        let installer = NodeInstaller::new(node_dir);

        let node_request = match &hook.language_request {
            LanguageRequest::Any => &NodeRequest::Any,
            LanguageRequest::Node(node_request) => node_request,
            _ => unreachable!(),
        };
        let node = installer.install(node_request).await?;

        let mut info = InstallInfo::new(
            hook.language,
            hook.dependencies().clone(),
            &store.hooks_dir(),
        );

        let lts = serde_json::to_string(&node.version().lts).context("Failed to serialize LTS")?;
        info.with_toolchain(node.node().to_path_buf());
        info.with_language_version(node.version().version.clone());
        info.with_extra(EXTRA_KEY_LTS, &lts);

        // 2. Create env
        let bin_dir = bin_dir(&info.env_path);
        let lib_dir = lib_dir(&info.env_path);
        fs_err::tokio::create_dir_all(&bin_dir).await?;
        fs_err::tokio::create_dir_all(&lib_dir).await?;

        // TODO: do we really need to create a symlink for `node` and `npm`?
        //   What about adding them to PATH directly?
        // Create symlink or copy on Windows
        create_symlink_or_copy(
            node.node(),
            &bin_dir.join("node").with_extension(EXE_EXTENSION),
        )
        .await?;

        // 3. Install dependencies
        let deps = if let Some(repo) = hook.repo_path() {
            let mut deps = hook.additional_dependencies.clone();
            deps.insert(repo.to_string_lossy().to_string());
            Cow::Owned::<FxHashSet<_>>(deps)
        } else {
            Cow::Borrowed(&hook.additional_dependencies)
        };
        if deps.is_empty() {
            debug!("No dependencies to install");
        } else {
            // npm install <folder>:
            // If <folder> sits inside the root of your project, its dependencies will be installed
            // and may be hoisted to the top-level node_modules as they would for other types of dependencies.
            // If <folder> sits outside the root of your project, npm will not install the package dependencies
            // in the directory <folder>, but it will create a symlink to <folder>.
            //
            // NOTE: If you want to install the content of a directory like a package from the registry
            // instead of creating a link, you would need to use the --install-links option.

            // `npm` is a script uses `/usr/bin/env node`, we need add `bin_dir` to PATH
            // so that `npm` can find `node`.
            let new_path = prepend_paths(&[&bin_dir]).context("Failed to join PATH")?;

            Cmd::new(node.npm(), "npm install")
                .arg("install")
                .arg("-g")
                .arg("--no-progress")
                .arg("--no-save")
                .arg("--no-fund")
                .arg("--no-audit")
                .arg("--install-links")
                .args(&*deps)
                .env("PATH", new_path)
                .env(EnvVars::NPM_CONFIG_PREFIX, &info.env_path)
                .env_remove(EnvVars::NPM_CONFIG_USERCONFIG)
                .env(EnvVars::NODE_PATH, &lib_dir)
                .check(true)
                .output()
                .await?;
        }

        Ok(InstalledHook::Installed {
            hook,
            info: Arc::new(info),
        })
    }

    async fn check_health(&self) -> Result<()> {
        todo!()
    }

    async fn run(
        &self,
        hook: &InstalledHook,
        filenames: &[&String],
        _store: &Store,
    ) -> Result<(i32, Vec<u8>)> {
        let env_dir = hook.env_path().expect("Node must have env path");
        let new_path = prepend_paths(&[&bin_dir(env_dir)]).context("Failed to join PATH")?;

        let entry = hook.entry.parsed()?;
        let run = async move |batch: Vec<String>| {
            // Npm install scripts as `xxx.cmd` on Windows, we use `which::which` find the
            // real command name `xxx.cmd` from `xxx`.
            let mut cmd = if cfg!(windows) {
                if let Some(path) = which::which_in_global(&entry[0], Some(&new_path))
                    .map_or(None, |mut p| p.next())
                {
                    Cmd::new(path, "node hook")
                } else {
                    Cmd::new(&entry[0], "node hook")
                }
            } else {
                Cmd::new(&entry[0], "node hook")
            };

            let mut output = cmd
                .args(&entry[1..])
                .env("PATH", &new_path)
                .env(EnvVars::NPM_CONFIG_PREFIX, env_dir)
                .env_remove(EnvVars::NPM_CONFIG_USERCONFIG)
                .env(EnvVars::NODE_PATH, lib_dir(env_dir))
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
