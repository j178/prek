use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use prek_consts::env_vars::{EnvVars, EnvVarsRead};
use tracing::debug;

use super::installer::{MiseInstaller, MiseResult, is_supported_version};
use super::{MiseRequest, inherited_mise_vars, is_mise_var};
use crate::cli::reporter::HookInstallReporter;
use crate::fs::{PathClean, expand_tilde};
use crate::hook::{Hook, InstallInfo, InstalledHook};
use crate::languages::{ExecutionEnvironment, LanguageBackend};
use crate::process::Cmd;
use crate::store::{Store, ToolBucket};

const MISE_ARGS: &[&str] = &["--yes"];
const MISE_ENVIRONMENT_KEY: &str = "mise_environment";
const ISOLATED_ENVIRONMENT: &str = "isolated-v1";
const REPOSITORY_ENVIRONMENT: &str = "repository-v1";

/// Mutable mise state stays inside the hook environment because mise's install
/// identity does not include every backend option. Equivalent hooks reuse the
/// entire environment through prek's normal environment cache.
#[derive(Debug)]
struct MiseEnvironment {
    data: PathBuf,
    cache: PathBuf,
    config: PathBuf,
    state: PathBuf,
    system_config: PathBuf,
    system_data: PathBuf,
    temp: PathBuf,
}

impl MiseEnvironment {
    fn new(env_path: &Path) -> Self {
        let root = env_path.join("mise");
        Self {
            data: root.join("data"),
            cache: root.join("cache"),
            config: root.join("config"),
            state: root.join("state"),
            system_config: root.join("system-config"),
            system_data: root.join("system-data"),
            temp: root.join("tmp"),
        }
    }

    async fn create(&self) -> Result<()> {
        for path in [
            &self.data,
            &self.cache,
            &self.config,
            &self.state,
            &self.system_config,
            &self.system_data,
            &self.temp,
        ] {
            fs_err::tokio::create_dir_all(path).await?;
        }
        Ok(())
    }

    fn vars(&self) -> [(&'static str, &OsStr); 9] {
        [
            (EnvVars::MISE_DATA_DIR, self.data.as_os_str()),
            (EnvVars::MISE_CACHE_DIR, self.cache.as_os_str()),
            (EnvVars::MISE_CONFIG_DIR, self.config.as_os_str()),
            (EnvVars::MISE_STATE_DIR, self.state.as_os_str()),
            (
                EnvVars::MISE_SYSTEM_CONFIG_DIR,
                self.system_config.as_os_str(),
            ),
            (EnvVars::MISE_SYSTEM_DATA_DIR, self.system_data.as_os_str()),
            (EnvVars::MISE_TMP_DIR, self.temp.as_os_str()),
            (EnvVars::MISE_SHARED_INSTALL_DIRS, OsStr::new("")),
            (EnvVars::MISE_SYSTEM_DEPS, OsStr::new("warn")),
        ]
    }

    fn isolation_vars() -> [(&'static str, &'static OsStr); 3] {
        [
            (EnvVars::MISE_NO_CONFIG, OsStr::new("1")),
            (EnvVars::MISE_NO_ENV, OsStr::new("1")),
            (EnvVars::MISE_NO_HOOKS, OsStr::new("1")),
        ]
    }

    fn apply_to_command(
        &self,
        command: &mut Cmd,
        configuration: &MiseConfiguration,
        command_kind: MiseCommandKind,
    ) -> Result<()> {
        remove_inherited_mise_vars_from_command(command);
        command.envs(self.vars());
        configuration.apply_to_command(command, command_kind)?;
        Ok(())
    }

    fn apply_to_environment(&self, environment: &mut ExecutionEnvironment) {
        remove_inherited_mise_vars_from_environment(environment);
        environment.envs(self.vars()).envs(Self::isolation_vars());
    }
}

#[derive(Debug)]
enum MiseConfiguration {
    Isolated(PathBuf),
    Repository { root: PathBuf, has_lockfile: bool },
}

impl MiseConfiguration {
    fn for_hook(hook: &Hook, cwd: &Path) -> Self {
        if let Some(root) = hook.repo_path()
            && root.join("mise.toml").is_file()
        {
            return Self::Repository {
                root: root.to_path_buf(),
                has_lockfile: root.join("mise.lock").is_file(),
            };
        }
        Self::Isolated(cwd.to_path_buf())
    }

    fn cwd(&self) -> &Path {
        match self {
            Self::Isolated(cwd) => cwd,
            Self::Repository { root, .. } => root,
        }
    }

    fn loads_repository_manifest(&self) -> bool {
        matches!(self, Self::Repository { .. })
    }

    fn apply_to_command(&self, command: &mut Cmd, command_kind: MiseCommandKind) -> Result<()> {
        let ceiling = std::env::join_paths([self.cwd()])
            .context("Failed to isolate mise from working directory configuration")?;
        command.env(EnvVars::MISE_CEILING_PATHS, ceiling);

        match self {
            Self::Isolated(_) => {
                command
                    .envs(MiseEnvironment::isolation_vars())
                    .env(EnvVars::MISE_LOCKFILE, "0")
                    .env(EnvVars::MISE_LOCKED, "0");
            }
            Self::Repository { root, has_lockfile } => {
                command
                    .env(EnvVars::MISE_GLOBAL_CONFIG_FILE, root.join("mise.toml"))
                    .env(EnvVars::MISE_GLOBAL_CONFIG_ROOT, root)
                    .env(EnvVars::MISE_OVERRIDE_CONFIG_FILENAMES, "mise.toml")
                    .env(EnvVars::MISE_OVERRIDE_TOOL_VERSIONS_FILENAMES, "none")
                    .env(EnvVars::MISE_AUTO_ENV, "0")
                    .env(EnvVars::MISE_ENV, "")
                    .env(EnvVars::MISE_NO_HOOKS, "1");

                match command_kind {
                    MiseCommandKind::AdditionalDependencies => {
                        command
                            .env(EnvVars::MISE_LOCKFILE, "0")
                            .env(EnvVars::MISE_LOCKED, "0");
                    }
                    MiseCommandKind::Configuration if *has_lockfile => {
                        command
                            .env(EnvVars::MISE_LOCKFILE, "1")
                            .env(EnvVars::MISE_LOCKED, "1");
                    }
                    MiseCommandKind::Configuration => {
                        command.env(EnvVars::MISE_LOCKFILE, "0");
                    }
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Copy, Clone)]
enum MiseCommandKind {
    Configuration,
    AdditionalDependencies,
}

async fn install_tools(
    mise: &Path,
    environment: &MiseEnvironment,
    configuration: &MiseConfiguration,
    tools: Option<&[String]>,
    command_kind: MiseCommandKind,
) -> Result<()> {
    let mut command = Cmd::new(mise);
    command
        .current_dir(configuration.cwd())
        .args(MISE_ARGS)
        .arg("install");
    if let Some(tools) = tools {
        command.arg("--").args(tools);
    }
    command.check(true);
    environment.apply_to_command(&mut command, configuration, command_kind)?;
    command.output().await?;
    Ok(())
}

fn remove_inherited_mise_vars_from_command(command: &mut Cmd) {
    for key in inherited_mise_vars() {
        command.env_remove(key);
    }
}

fn remove_inherited_mise_vars_from_environment(environment: &mut ExecutionEnvironment) {
    for key in inherited_mise_vars() {
        environment.env_remove(key);
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
        reporter: &HookInstallReporter,
    ) -> Result<InstalledHook> {
        validate_additional_dependencies(&hook)?;
        let progress = reporter.on_install_start(&hook);
        let installer = MiseInstaller::new(store.tools_path(ToolBucket::Mise));
        let request: &MiseRequest = hook.language_request.version();
        let mise = installer
            .install(store, request, hook.language_request.allows_download())
            .await
            .context("Failed to install mise")?;

        let mut info = InstallInfo::new(&hook, &store.hooks_dir())?;
        info.with_toolchain(mise.mise().to_path_buf())
            .with_language_version(mise.version().clone());

        let environment = MiseEnvironment::new(&info.env_path);
        environment.create().await?;
        let install_cwd = hook.repo_path().unwrap_or(hook.work_dir());
        let configuration = MiseConfiguration::for_hook(&hook, install_cwd);
        info.with_extra(
            MISE_ENVIRONMENT_KEY,
            if configuration.loads_repository_manifest() {
                REPOSITORY_ENVIRONMENT
            } else {
                ISOLATED_ENVIRONMENT
            },
        );

        if configuration.loads_repository_manifest() {
            debug!("Installing tools from repository mise.toml");
            install_tools(
                mise.mise(),
                &environment,
                &configuration,
                None,
                MiseCommandKind::Configuration,
            )
            .await
            .context("Failed to install tools from repository mise.toml")?;
        }

        if !hook.additional_dependencies.is_empty() {
            debug!(deps = ?hook.additional_dependencies, "Installing mise tools");
            let tools = normalized_tools(&hook)?;
            install_tools(
                mise.mise(),
                &environment,
                &configuration,
                Some(&tools),
                MiseCommandKind::AdditionalDependencies,
            )
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
        match info.get_extra(MISE_ENVIRONMENT_KEY).map(String::as_str) {
            Some(ISOLATED_ENVIRONMENT | REPOSITORY_ENVIRONMENT) => {}
            _ => anyhow::bail!("mise environment format is outdated"),
        }

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
        _store: &Store,
        hook: &InstalledHook,
    ) -> Result<ExecutionEnvironment> {
        validate_additional_dependencies(hook)?;
        let info = hook.install_info().context("mise must be installed")?;
        let path = activation_base_path(hook, &info.toolchain)?;

        let mut environment = ExecutionEnvironment::new();
        environment.set_path(path);
        MiseEnvironment::new(&info.env_path).apply_to_environment(&mut environment);
        Ok(environment)
    }

    async fn prepare_execution_environment(
        &self,
        hook: &InstalledHook,
        cwd: &Path,
        environment: &mut ExecutionEnvironment,
    ) -> Result<()> {
        let ceiling = std::env::join_paths([cwd])
            .context("Failed to isolate mise from working directory configuration")?;
        environment.env(EnvVars::MISE_CEILING_PATHS, ceiling);

        let info = hook.install_info().context("mise must be installed")?;
        let mise_environment = MiseEnvironment::new(&info.env_path);
        let configuration = MiseConfiguration::for_hook(hook, cwd);
        match (
            info.get_extra(MISE_ENVIRONMENT_KEY).map(String::as_str),
            configuration.loads_repository_manifest(),
        ) {
            (Some(REPOSITORY_ENVIRONMENT), false) => {
                anyhow::bail!("repository mise.toml is missing")
            }
            (Some(ISOLATED_ENVIRONMENT), true) => {
                anyhow::bail!("mise environment does not include the repository mise.toml")
            }
            _ => {}
        }
        if hook.additional_dependencies.is_empty() && !configuration.loads_repository_manifest() {
            return Ok(());
        }

        let tools = normalized_tools(hook)?;
        let base_path = activation_base_path(hook, &info.toolchain)?;
        // Only tool specs pass through mise's UTF-8 CLI; hook argv is spawned directly later.
        let mut command = Cmd::new(&info.toolchain);
        command
            .current_dir(configuration.cwd())
            .envs(
                hook.env
                    .iter()
                    .filter(|(key, _)| !is_mise_var(OsStr::new(key))),
            )
            .env(EnvVars::PATH, &base_path)
            .args(MISE_ARGS)
            .arg("env")
            .arg("--json");
        if !tools.is_empty() {
            command.arg("--").args(&tools);
        }
        command.check(true);
        let command_kind = if tools.is_empty() {
            MiseCommandKind::Configuration
        } else {
            MiseCommandKind::AdditionalDependencies
        };
        mise_environment.apply_to_command(&mut command, &configuration, command_kind)?;
        let output = command
            .output()
            .await
            .context("Failed to activate mise tools")?;

        let mut activated: BTreeMap<String, String> =
            serde_json::from_slice(&output.stdout).context("Failed to parse mise environment")?;
        let path_key = activated
            .keys()
            .find(|key| is_path_key(key))
            .cloned()
            .context("mise environment did not include PATH")?;
        let activated_path = activated
            .remove(&path_key)
            .context("mise environment did not include PATH")?;
        let mise_bin = info
            .toolchain
            .parent()
            .context("mise executable must have a parent directory")?;
        let activated_path = merge_activated_path(mise_bin, &activated_path, &base_path)?;
        activated.retain(|key, _| !is_mise_var(OsStr::new(key)));
        environment.envs(&activated).set_path(activated_path);

        Ok(())
    }
}

fn validate_additional_dependencies(hook: &Hook) -> Result<()> {
    if hook.repo_path().is_none() {
        for tool in &hook.additional_dependencies {
            let (_, Some(version)) = split_tool_version(tool) else {
                continue;
            };
            let Some(path) = version.strip_prefix("path:") else {
                continue;
            };
            if !expand_tilde(PathBuf::from(path)).is_absolute() {
                anyhow::bail!(
                    "local mise hook dependency `{tool}` must use an absolute `path:` version"
                );
            }
        }
    }
    Ok(())
}

fn activation_base_path(hook: &Hook, mise: &Path) -> Result<std::ffi::OsString> {
    let bin_dir = mise
        .parent()
        .context("mise executable must have a parent directory")?;
    let base_path = hook
        .env
        .iter()
        .find_map(|(key, value)| is_path_key(key).then_some(OsStr::new(value)))
        .map(ToOwned::to_owned)
        .or_else(|| EnvVars.var_os(EnvVars::PATH));
    std::env::join_paths(
        std::iter::once(bin_dir.to_path_buf()).chain(
            base_path
                .as_ref()
                .into_iter()
                .flat_map(std::env::split_paths),
        ),
    )
    .context("Failed to join mise PATH")
}

fn is_path_key(key: &str) -> bool {
    #[cfg(windows)]
    {
        key.eq_ignore_ascii_case(EnvVars::PATH)
    }
    #[cfg(not(windows))]
    {
        key == EnvVars::PATH
    }
}

fn merge_activated_path(
    mise_bin: &Path,
    activated: &str,
    base: &OsStr,
) -> Result<std::ffi::OsString> {
    let mut paths = vec![mise_bin.to_path_buf()];
    for path in std::env::split_paths(OsStr::new(activated)) {
        if !paths.contains(&path) {
            paths.push(path);
        }
    }
    if base.to_str().is_none() {
        for path in std::env::split_paths(base) {
            if !paths.contains(&path) {
                paths.push(path);
            }
        }
    }
    std::env::join_paths(paths).context("Failed to join activated mise PATH")
}

fn normalized_tools(hook: &Hook) -> Result<Vec<String>> {
    let base = hook.repo_path().unwrap_or(hook.work_dir());
    hook.additional_dependencies
        .iter()
        .map(|tool| tool_with_version(tool, base))
        .collect()
}

fn tool_with_version(tool: &str, base: &Path) -> Result<String> {
    let (backend, version) = split_tool_version(tool);
    let version = version.unwrap_or("latest");
    let version = if let Some(path) = version.strip_prefix("path:") {
        let path = expand_tilde(PathBuf::from(path));
        let path = if path.is_absolute() {
            path
        } else {
            base.join(path).clean()
        };
        let path = path
            .to_str()
            .context("mise path tool must have a UTF-8 path")?;
        #[cfg(windows)]
        let path = path.replace('\\', "/");
        format!("path:{path}")
    } else {
        version.to_string()
    };
    Ok(format!("{backend}@{version}"))
}

fn split_tool_version(tool: &str) -> (&str, Option<&str>) {
    let Some((left, right)) = tool.split_once('@') else {
        return (tool, None);
    };
    if left.is_empty() {
        return right
            .split_once('@')
            .map(|(name, version)| {
                (
                    &tool[..=name.len()],
                    (!version.is_empty()).then_some(version),
                )
            })
            .unwrap_or((tool, None));
    }
    if left.ends_with(':') {
        return right
            .split_once('@')
            .map(|(name, version)| {
                (
                    &tool[..=(left.len() + name.len())],
                    (!version.is_empty()).then_some(version),
                )
            })
            .unwrap_or((tool, None));
    }
    (left, (!right.is_empty()).then_some(right))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    #[cfg(unix)]
    use std::{ffi::OsString, os::unix::ffi::OsStringExt, path::PathBuf};

    use anyhow::Result;

    use super::{merge_activated_path, tool_with_version};

    #[test]
    fn gives_unversioned_tools_an_explicit_latest_version() -> Result<()> {
        let cases = [
            ("node", "node@latest"),
            ("node@", "node@latest"),
            ("node@22", "node@22"),
            (
                "github:ajeetdsouza/zoxide",
                "github:ajeetdsouza/zoxide@latest",
            ),
            ("npm:@antfu/ni", "npm:@antfu/ni@latest"),
            ("npm:@antfu/ni@1", "npm:@antfu/ni@1"),
            ("@biomejs/biome", "@biomejs/biome@latest"),
            ("@biomejs/biome@2", "@biomejs/biome@2"),
        ];

        for (tool, expected) in cases {
            assert_eq!(tool_with_version(tool, Path::new("unused"))?, expected);
        }
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn resolves_relative_path_versions_from_the_hook_repository() -> Result<()> {
        assert_eq!(
            tool_with_version("node@path:../node", Path::new("/repo/hook"))?,
            "node@path:/repo/node"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn preserves_a_non_utf8_base_path() -> Result<()> {
        let base = OsString::from_vec(b"/non-utf8-\xff/bin:/usr/bin".to_vec());
        let path = merge_activated_path(Path::new("/mise/bin"), "/tool/bin", &base)?;
        let paths = std::env::split_paths(&path).collect::<Vec<_>>();

        assert_eq!(
            paths,
            vec![
                PathBuf::from("/mise/bin"),
                PathBuf::from("/tool/bin"),
                PathBuf::from(OsString::from_vec(b"/non-utf8-\xff/bin".to_vec())),
                PathBuf::from("/usr/bin")
            ]
        );
        Ok(())
    }
}
