use crate::hook::InstalledHook;
use crate::hook::{Hook, InstallInfo};
use crate::languages::node::installer::{EXTRA_KEY_LTS, NodeInstaller};
use crate::languages::{Error, LanguageImpl};
use crate::process::Cmd;
use crate::store::{Store, ToolBucket};
use std::collections::HashMap;
use tracing::debug;

#[derive(Debug, Copy, Clone)]
pub(crate) struct Node;

impl LanguageImpl for Node {
    async fn install(&self, hook: &Hook, store: &Store) -> Result<InstalledHook, Error> {
        // 1. Install node
        //   1) Find from `$PREFLIGIT_HOME/tools/node`
        //   2) Find from system
        //   3) Download from remote
        // 2. Create env
        //   1) Create a directory for the env
        //   2) Create a symlink to the node binary in the env directory
        // 3. Install dependencies

        // 1. Install node
        let node_dir = store.tools_path(ToolBucket::Node);
        let installer = NodeInstaller::new(node_dir);
        let node = installer.install(&hook.language_request).await?;

        let mut info = InstallInfo::new(hook.language, hook.dependencies().to_vec(), store);
        info.clear_env_path().await?;

        info.with_toolchain(node.node().to_path_buf());
        info.with_language_version(node.version().version.clone());
        info.with_extra(EXTRA_KEY_LTS, &node.version().lts.to_string());

        // 2. Create env
        // TODO: windows different path?
        fs_err::tokio::create_dir_all(info.env_path.join("bin")).await?;
        fs_err::tokio::create_dir_all(info.env_path.join("lib/node_modules")).await?;

        // 3. Install dependencies
        let pkg = if let Some(repo_path) = hook.repo_path() {
            Cmd::new(&*node.npm(), "npm install")
                .arg("install")
                .arg("--include=dev")
                .arg("--include=prod")
                .arg("--no-progress")
                .arg("--no-save")
                .arg("--no-fund")
                .arg("--no-audit")
                .current_dir(repo_path)
                .check(true)
                .output()
                .await?;
            let output = Cmd::new(&*node.npm(), "npm pack")
                .arg("pack")
                .current_dir(repo_path)
                .check(true)
                .output()
                .await?;

            // Clean up node_modules
            fs_err::tokio::remove_dir_all(repo_path.join("node_modules")).await?;

            let output_str = String::from_utf8_lossy(&output.stdout);
            let pkg_name = output_str.trim();
            Some(repo_path.join(pkg_name))
        } else {
            None
        };

        let mut deps = hook.dependencies().to_vec();
        if let Some(pkg) = pkg {
            deps.insert(0, pkg.to_string_lossy().to_string());
        }
        if !deps.is_empty() {
            Cmd::new(&*node.npm(), "npm install")
                .arg("install")
                .arg("-g") // TODO?
                .arg("--no-progress")
                .arg("--no-save")
                .arg("--no-fund")
                .arg("--no-audit")
                .args(&deps)
                .env("npm_config_prefix", &info.env_path)
                .check(true)
                .output()
                .await?;
        } else {
            debug!("No dependencies to install");
        }

        Ok(InstalledHook::Installed {
            hook: hook.clone(),
            info,
        })
    }

    async fn check_health(&self) -> Result<(), Error> {
        todo!()
    }

    async fn run(
        &self,
        _hook: &InstalledHook,
        _filenames: &[&String],
        _env_vars: &HashMap<&'static str, String>,
        _store: &Store,
    ) -> Result<(i32, Vec<u8>), Error> {
        Ok((0, Vec::new()))
    }
}
