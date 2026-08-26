use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use prek_consts::env_vars::{EnvVars, EnvVarsRead};
use tracing::debug;

use super::installer::{MiseInstaller, MiseResult, is_supported_version};
use super::{inherited_mise_vars, is_mise_var, mise_ceiling};
use crate::cli::reporter::HookInstallReporter;
use crate::hook::{Hook, InstallInfo, InstalledHook};
use crate::languages::version::SemverRequest;
use crate::languages::{ExecutionEnvironment, LanguageBackend, is_path_env};
use crate::process::Cmd;
use crate::store::{Store, ToolBucket};

/// Mutable mise state stays inside the hook environment because mise's install
/// identity does not include every backend option. Equivalent hooks reuse the
/// entire environment through prek's normal environment cache.
#[derive(Debug)]
struct MiseEnvironment {
    root: PathBuf,
}

impl MiseEnvironment {
    fn new(env_path: &Path) -> Self {
        Self {
            root: env_path.join("mise"),
        }
    }

    fn vars(&self) -> [(&'static str, OsString); 9] {
        let path = |name| self.root.join(name).into_os_string();
        [
            (EnvVars::MISE_DATA_DIR, path("data")),
            (EnvVars::MISE_CACHE_DIR, path("cache")),
            (EnvVars::MISE_CONFIG_DIR, path("config")),
            (EnvVars::MISE_STATE_DIR, path("state")),
            (EnvVars::MISE_SYSTEM_CONFIG_DIR, path("system-config")),
            (EnvVars::MISE_SYSTEM_DATA_DIR, path("system-data")),
            (EnvVars::MISE_TMP_DIR, path("tmp")),
            (EnvVars::MISE_NO_CONFIG, OsString::from("1")),
            (EnvVars::MISE_SYSTEM_DEPS, OsString::from("warn")),
        ]
    }

    fn command(&self, mise: &Path, cwd: &Path) -> Result<Cmd> {
        let mut command = Cmd::new(mise);
        for key in inherited_mise_vars() {
            command.env_remove(key);
        }
        command
            .current_dir(cwd)
            .envs(self.vars())
            .env(EnvVars::MISE_CEILING_PATHS, mise_ceiling(cwd)?)
            .arg("--yes")
            .check(true);
        Ok(command)
    }

    fn apply_to_environment(&self, environment: &mut ExecutionEnvironment) {
        for key in inherited_mise_vars() {
            environment.env_remove(key);
        }
        environment.envs(self.vars());
    }
}

#[derive(Debug, Copy, Clone)]
pub(crate) struct Mise;

#[async_trait::async_trait(?Send)]
impl LanguageBackend for Mise {
    async fn install(
        &self,
        store: &Store,
        hook: Arc<Hook>,
        install_cwd: &Path,
        reporter: &HookInstallReporter,
    ) -> Result<InstalledHook> {
        let progress = reporter.on_install_start(&hook);
        let installer = MiseInstaller::new(store.tools_path(ToolBucket::Mise));
        let request: &SemverRequest = hook.language_request.version();
        let mise = installer
            .install(store, request, hook.language_request.toolchain_policy())
            .await
            .context("Failed to install mise")?;

        let mut info = InstallInfo::new(&hook, &store.hooks_dir())?;
        info.with_toolchain(mise.mise().to_path_buf())
            .with_language_version(mise.version().clone());

        let environment = MiseEnvironment::new(&info.env_path);

        // TODO(#2022): Support provisioning remote hooks from the repository's `mise.toml`.
        if !hook.additional_dependencies.is_empty() {
            debug!(deps = ?hook.additional_dependencies, "Installing mise tools");
            let mut command = environment.command(mise.mise(), install_cwd)?;
            command
                .arg("install")
                .arg("--")
                .args(tools_with_versions(&hook.additional_dependencies));
            command
                .output()
                .await
                .context("Failed to install mise tools")?;
        }

        info.persist_env_path();
        reporter.on_install_complete(progress);

        Ok(InstalledHook::Installed {
            hook,
            info: Arc::new(info),
        })
    }

    async fn check_health(&self, info: &InstallInfo) -> Result<()> {
        let mise = MiseResult::from_executable(info.toolchain.clone())
            .await
            .context("Failed to query mise version")?;
        if mise.version() != &info.language_version {
            anyhow::bail!(
                "mise version mismatch: expected {}, found {}",
                info.language_version,
                mise.version()
            );
        }
        if !is_supported_version(mise.version()) {
            anyhow::bail!("mise {} is no longer supported", mise.version());
        }
        Ok(())
    }

    fn execution_environment(
        &self,
        store: &Store,
        hook: &InstalledHook,
    ) -> Result<ExecutionEnvironment> {
        let info = hook.install_info().context("mise must be installed")?;

        let mut environment = ExecutionEnvironment::new();
        // Only prepend prek's managed bin directory. System mise is already on PATH, and moving
        // its parent would also reorder unrelated executables in that directory.
        if info
            .toolchain
            .starts_with(store.tools_path(ToolBucket::Mise))
        {
            environment.set_path(managed_mise_path(&info.toolchain)?);
        }
        MiseEnvironment::new(&info.env_path).apply_to_environment(&mut environment);
        Ok(environment)
    }

    async fn prepare_execution_environment(
        &self,
        hook: &InstalledHook,
        cwd: &Path,
        environment: &mut ExecutionEnvironment,
    ) -> Result<()> {
        environment.env(EnvVars::MISE_CEILING_PATHS, mise_ceiling(cwd)?);

        if hook.additional_dependencies.is_empty() {
            return Ok(());
        }

        let info = hook.install_info().context("mise must be installed")?;
        let mise_environment = MiseEnvironment::new(&info.env_path);
        let tool_cwd = tool_cwd(hook);
        // Backends can contribute dynamic environment variables and PATH entries, so activation
        // must be delegated to the selected mise CLI. Hook argv never crosses its UTF-8 boundary.
        let mut command = mise_environment.command(&info.toolchain, tool_cwd)?;
        if let Some(path) = environment.language_path() {
            command.env(EnvVars::PATH, path);
        }
        command
            .arg("env")
            .arg("--json")
            .arg("--")
            .args(tools_with_versions(&hook.additional_dependencies));
        let output = command
            .output()
            .await
            .context("Failed to activate mise tools")?;

        let mut activated: BTreeMap<String, String> =
            serde_json::from_slice(&output.stdout).context("Failed to parse mise environment")?;
        let activated_path = activated
            .iter()
            .find_map(|(key, value)| {
                if is_path_env(key) {
                    Some(value.clone())
                } else {
                    None
                }
            })
            .context("mise environment did not include PATH")?;
        // TODO(#2022): Preserve non-UTF-8 PATH entries omitted by `mise env --json`.
        activated.retain(|key, _| !is_path_env(key) && !is_mise_var(OsStr::new(key)));
        environment.envs(&activated).set_path(activated_path);

        Ok(())
    }
}

fn tool_cwd(hook: &Hook) -> &Path {
    if let Some(repo_path) = hook.repo_path() {
        repo_path
    } else {
        hook.work_dir()
    }
}

/// Prepends a managed mise CLI to the inherited PATH.
fn managed_mise_path(mise: &Path) -> Result<OsString> {
    let bin_dir = mise
        .parent()
        .context("mise executable must have a parent directory")?
        .to_path_buf();
    let base_path = EnvVars.var_os(EnvVars::PATH);
    std::env::join_paths(
        std::iter::once(bin_dir).chain(
            base_path
                .as_ref()
                .into_iter()
                .flat_map(std::env::split_paths),
        ),
    )
    .context("Failed to join mise PATH")
}

fn tools_with_versions(tools: &[String]) -> impl Iterator<Item = String> + '_ {
    tools.iter().map(|tool| tool_with_version(tool))
}

fn tool_with_version(tool: &str) -> String {
    let (backend, version) = split_tool_version(tool);
    let version = version.unwrap_or("latest");
    format!("{backend}@{version}")
}

fn split_tool_version(tool: &str) -> (&str, Option<&str>) {
    let Some((left, right)) = tool.split_once('@') else {
        return (tool, None);
    };
    let (backend, version) = if left.is_empty() {
        let Some((name, version)) = right.split_once('@') else {
            return (tool, None);
        };
        (&tool[..=name.len()], version)
    } else if left.ends_with(':') {
        let Some((name, version)) = right.split_once('@') else {
            return (tool, None);
        };
        (&tool[..=(left.len() + name.len())], version)
    } else {
        (left, right)
    };
    let version = if version.is_empty() {
        None
    } else {
        Some(version)
    };
    (backend, version)
}

#[cfg(test)]
mod tests {
    use super::tool_with_version;

    #[test]
    fn gives_unversioned_tools_an_explicit_latest_version() {
        let cases = [
            ("node", "node@latest"),
            ("node@", "node@latest"),
            ("node@22", "node@22"),
            (
                "github:ajeetdsouza/zoxide",
                "github:ajeetdsouza/zoxide@latest",
            ),
            (
                "ubi:BurntSushi/ripgrep[exe=rg]",
                "ubi:BurntSushi/ripgrep[exe=rg]@latest",
            ),
            ("npm:@antfu/ni", "npm:@antfu/ni@latest"),
            ("npm:@antfu/ni@1", "npm:@antfu/ni@1"),
            ("@biomejs/biome", "@biomejs/biome@latest"),
            ("@biomejs/biome@2", "@biomejs/biome@2"),
            ("node@path:../node", "node@path:../node"),
        ];

        for (tool, expected) in cases {
            assert_eq!(tool_with_version(tool), expected);
        }
    }
}
