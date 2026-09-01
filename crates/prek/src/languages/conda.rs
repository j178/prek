use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use prek_consts::env_vars::{EnvVars, EnvVarsRead};
use prek_consts::prepend_paths;
use tracing::debug;

use crate::cli::reporter::HookInstallReporter;
use crate::hook::{Hook, InstallInfo, InstalledHook};
use crate::languages::{ExecutionEnvironment, LanguageBackend};
use crate::process::Cmd;
use crate::store::Store;

#[derive(Debug, Copy, Clone)]
pub(crate) struct Conda;

const INSTALLER_KEY: &str = "conda-installer";

#[derive(Debug, Copy, Clone, PartialEq, Eq, strum::AsRefStr, strum::EnumString)]
#[strum(serialize_all = "lowercase")]
enum CondaInstaller {
    Pixi,
    Micromamba,
    Mamba,
    Conda,
}

impl CondaInstaller {
    const AUTO_ORDER: [Self; 4] = [Self::Pixi, Self::Micromamba, Self::Mamba, Self::Conda];

    fn resolve(
        env_vars: &impl EnvVarsRead,
        find_executable: impl Fn(&str) -> Option<PathBuf>,
    ) -> Result<(Self, PathBuf)> {
        let requested = match env_vars.var(EnvVars::PREK_CONDA_INSTALLER) {
            Ok(value) if value == "auto" => None,
            Ok(value) => {
                let Ok(installer) = value.parse::<Self>() else {
                    bail!(
                        "Invalid value for {}: {value:?}. Expected auto, pixi, micromamba, mamba, or conda",
                        EnvVars::PREK_CONDA_INSTALLER,
                    );
                };
                Some(installer)
            }
            Err(std::env::VarError::NotPresent) => {
                // TODO: Remove these pre-commit compatibility variables in the next breaking release.
                if env_vars.is_set(EnvVars::PRE_COMMIT_USE_MICROMAMBA) {
                    Some(Self::Micromamba)
                } else if env_vars.is_set(EnvVars::PRE_COMMIT_USE_MAMBA) {
                    Some(Self::Mamba)
                } else {
                    None
                }
            }
            Err(std::env::VarError::NotUnicode(value)) => {
                bail!(
                    "Invalid value for {}: {}. Expected auto, pixi, micromamba, mamba, or conda",
                    EnvVars::PREK_CONDA_INSTALLER,
                    value.display(),
                );
            }
        };

        if let Some(installer) = requested {
            let executable = find_executable(installer.as_ref()).with_context(|| {
                format!(
                    "{}={}, but `{}` was not found on PATH",
                    EnvVars::PREK_CONDA_INSTALLER,
                    installer.as_ref(),
                    installer.as_ref(),
                )
            })?;
            return Ok((installer, executable));
        }

        for installer in Self::AUTO_ORDER {
            if let Some(executable) = find_executable(installer.as_ref()) {
                return Ok((installer, executable));
            }
        }

        // TODO: Install a managed Pixi when no Conda installer is available, like mise and uv.
        bail!("No Conda installer found on PATH. Install one of: pixi, micromamba, mamba, conda");
    }

    async fn install(
        self,
        executable: &Path,
        hook: &Hook,
        install_cwd: &Path,
        env_path: &Path,
    ) -> Result<()> {
        match self {
            Self::Pixi => Self::install_with_pixi(executable, hook, install_cwd, env_path).await,
            Self::Micromamba | Self::Mamba | Self::Conda => {
                Self::install_with_conda(executable, hook, install_cwd, env_path).await
            }
        }
    }

    async fn install_with_conda(
        executable: &Path,
        hook: &Hook,
        install_cwd: &Path,
        env_path: &Path,
    ) -> Result<()> {
        let mut create_cmd = Cmd::new(executable);
        create_cmd
            .current_dir(install_cwd)
            .arg("create")
            .arg("-p")
            .arg(env_path);
        if hook.repo_path().is_some() {
            create_cmd.arg("--file").arg("environment.yml");
        }
        create_cmd
            .check(true)
            .output()
            .await
            .context("Failed to create Conda environment")?;

        if !hook.additional_dependencies.is_empty() {
            Cmd::new(executable)
                .current_dir(install_cwd)
                .arg("install")
                .arg("-p")
                .arg(env_path)
                .args(&hook.additional_dependencies)
                .check(true)
                .output()
                .await
                .context("Failed to install Conda dependencies")?;
        }

        Ok(())
    }

    async fn install_with_pixi(
        executable: &Path,
        hook: &Hook,
        install_cwd: &Path,
        env_path: &Path,
    ) -> Result<()> {
        let mut init_cmd = Cmd::new(executable);
        init_cmd.current_dir(install_cwd).arg("init");
        if hook.repo_path().is_some() {
            init_cmd
                .arg("--import")
                .arg(install_cwd.join("environment.yml"));
        }
        init_cmd
            .arg(env_path)
            .check(true)
            .output()
            .await
            .context("Failed to initialize Pixi environment")?;

        // The environment must remain below `env_path` so prek can reuse and remove it as one unit.
        let pixi_dir = env_path.join(".pixi");
        fs_err::tokio::create_dir_all(&pixi_dir).await?;
        fs_err::tokio::write(
            pixi_dir.join("config.toml"),
            "detached-environments = false\n",
        )
        .await?;

        let manifest = env_path.join("pixi.toml");
        let mut install_cmd = Cmd::new(executable);
        install_cmd.current_dir(install_cwd);
        if hook.additional_dependencies.is_empty() {
            install_cmd.arg("install");
        } else {
            install_cmd.arg("add").args(&hook.additional_dependencies);
        }
        install_cmd
            .arg("--manifest-path")
            .arg(manifest)
            .check(true)
            .output()
            .await
            .context("Failed to install Pixi environment")?;

        Ok(())
    }
}

#[async_trait::async_trait(?Send)]
impl LanguageBackend for Conda {
    async fn install(
        &self,
        store: &Store,
        hook: Arc<Hook>,
        install_cwd: &Path,
        reporter: &HookInstallReporter,
    ) -> Result<InstalledHook> {
        let progress = reporter.on_install_start(&hook);

        let mut info = InstallInfo::new(&hook, &store.hooks_dir())?;

        debug!(%hook, target = %info.env_path.display(), "Installing Conda environment");
        let (installer, executable) =
            CondaInstaller::resolve(&EnvVars, |name| which::which(name).ok())?;

        installer
            .install(&executable, &hook, install_cwd, &info.env_path)
            .await?;

        info.with_extra(INSTALLER_KEY, installer.as_ref());
        info.persist_env_path();

        reporter.on_install_complete(progress);

        Ok(InstalledHook::Installed {
            hook,
            info: Arc::new(info),
        })
    }

    async fn check_health(&self, info: &InstallInfo) -> Result<()> {
        check_environment(info)
    }

    fn execution_environment(
        &self,
        _store: &Store,
        hook: &InstalledHook,
    ) -> Result<ExecutionEnvironment> {
        let info = hook
            .install_info()
            .context("Conda hook must have installation information")?;
        let env_dir = installed_environment_path(info);
        let new_path = conda_path(&env_dir).context("Failed to join PATH")?;

        let mut environment = ExecutionEnvironment::new();
        environment
            .set_path(&new_path)
            .env(EnvVars::CONDA_PREFIX, &env_dir)
            .env_remove(EnvVars::PYTHONHOME)
            .env_remove(EnvVars::VIRTUAL_ENV);
        Ok(environment)
    }
}

fn check_environment(info: &InstallInfo) -> Result<()> {
    let env_dir = installed_environment_path(info);
    if !env_dir.is_dir() {
        bail!("Conda environment not found at {}", env_dir.display());
    }
    Ok(())
}

fn installed_environment_path(info: &InstallInfo) -> PathBuf {
    if info.get_extra(INSTALLER_KEY).map(String::as_str) == Some(CondaInstaller::Pixi.as_ref()) {
        info.env_path.join(".pixi").join("envs").join("default")
    } else {
        info.env_path.clone()
    }
}

fn conda_path(env_path: &Path) -> Result<std::ffi::OsString, std::env::JoinPathsError> {
    let paths = conda_path_dirs(env_path);
    let paths = paths.iter().map(PathBuf::as_path).collect::<Vec<_>>();
    prepend_paths(&paths)
}

fn conda_path_dirs(env_path: &Path) -> Vec<PathBuf> {
    if cfg!(windows) {
        vec![
            env_path.join("Library").join("bin"),
            env_path.join("Scripts"),
            env_path.to_path_buf(),
            env_path.join("bin"),
        ]
    } else {
        vec![env_path.join("bin")]
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use prek_consts::env_vars::EnvVars;

    use super::{CondaInstaller, check_environment};
    use crate::config::Language;
    use crate::hook::InstallInfo;

    #[test]
    fn auto_prefers_pixi() {
        let env_vars = EnvVars::from_map(&[(EnvVars::PREK_CONDA_INSTALLER, "auto")]);

        let (installer, executable) =
            CondaInstaller::resolve(&env_vars, |name| Some(PathBuf::from(name))).unwrap();

        assert_eq!(installer, CondaInstaller::Pixi);
        assert_eq!(executable, PathBuf::from("pixi"));
    }

    #[test]
    fn unset_installer_falls_back_to_first_available_executable() {
        let env_vars = EnvVars::from_map(&[]);

        let (installer, executable) = CondaInstaller::resolve(&env_vars, |name| {
            if name == "micromamba" {
                Some(PathBuf::from(name))
            } else {
                None
            }
        })
        .unwrap();

        assert_eq!(installer, CondaInstaller::Micromamba);
        assert_eq!(executable, PathBuf::from("micromamba"));
    }

    #[test]
    fn explicit_installer_overrides_auto_detection_and_legacy_variables() {
        let env_vars = EnvVars::from_map(&[
            (EnvVars::PREK_CONDA_INSTALLER, "mamba"),
            (EnvVars::PRE_COMMIT_USE_MICROMAMBA, "1"),
        ]);

        let (installer, executable) =
            CondaInstaller::resolve(&env_vars, |name| Some(PathBuf::from(name))).unwrap();

        assert_eq!(installer, CondaInstaller::Mamba);
        assert_eq!(executable, PathBuf::from("mamba"));
    }

    #[test]
    fn legacy_micromamba_takes_precedence_over_mamba() {
        let env_vars = EnvVars::from_map(&[
            (EnvVars::PRE_COMMIT_USE_MAMBA, "1"),
            (EnvVars::PRE_COMMIT_USE_MICROMAMBA, "1"),
        ]);

        let (installer, executable) =
            CondaInstaller::resolve(&env_vars, |name| Some(PathBuf::from(name))).unwrap();

        assert_eq!(installer, CondaInstaller::Micromamba);
        assert_eq!(executable, PathBuf::from("micromamba"));
    }

    #[test]
    fn legacy_mamba_is_used_when_new_selector_is_unset() {
        let env_vars = EnvVars::from_map(&[(EnvVars::PRE_COMMIT_USE_MAMBA, "1")]);

        let (installer, executable) =
            CondaInstaller::resolve(&env_vars, |name| Some(PathBuf::from(name))).unwrap();

        assert_eq!(installer, CondaInstaller::Mamba);
        assert_eq!(executable, PathBuf::from("mamba"));
    }

    #[test]
    fn invalid_installer_is_rejected() {
        let env_vars = EnvVars::from_map(&[(EnvVars::PREK_CONDA_INSTALLER, "sometimes")]);

        assert_eq!(
            CondaInstaller::resolve(&env_vars, |_| None)
                .unwrap_err()
                .to_string(),
            "Invalid value for PREK_CONDA_INSTALLER: \"sometimes\". Expected auto, pixi, micromamba, mamba, or conda",
        );
    }

    #[test]
    fn missing_explicit_installer_is_rejected() {
        let env_vars = EnvVars::from_map(&[(EnvVars::PREK_CONDA_INSTALLER, "micromamba")]);

        assert_eq!(
            CondaInstaller::resolve(&env_vars, |_| None)
                .unwrap_err()
                .to_string(),
            "PREK_CONDA_INSTALLER=micromamba, but `micromamba` was not found on PATH",
        );
    }

    #[test]
    fn auto_requires_an_available_installer() {
        let env_vars = EnvVars::from_map(&[]);

        assert_eq!(
            CondaInstaller::resolve(&env_vars, |_| None)
                .unwrap_err()
                .to_string(),
            "No Conda installer found on PATH. Install one of: pixi, micromamba, mamba, conda",
        );
    }

    #[test]
    fn pixi_environment_is_healthy_when_its_prefix_exists() {
        let temp_dir = tempfile::tempdir().unwrap();
        let mut info =
            InstallInfo::create(Language::Conda, None, Vec::new(), temp_dir.path()).unwrap();
        info.with_extra(super::INSTALLER_KEY, CondaInstaller::Pixi.as_ref());
        fs_err::create_dir_all(super::installed_environment_path(&info)).unwrap();

        assert!(check_environment(&info).is_ok());
    }
}
