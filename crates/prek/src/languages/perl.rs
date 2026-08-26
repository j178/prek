use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use prek_consts::env_vars::EnvVars;
use prek_consts::prepend_paths;
use tracing::debug;

use crate::cli::reporter::HookInstallReporter;
use crate::hook::{Hook, InstallInfo, InstalledHook};
use crate::languages::{ExecutionEnvironment, LanguageBackend};
use crate::process::Cmd;
use crate::store::Store;

#[derive(Debug, Copy, Clone)]
pub(crate) struct Perl;

#[async_trait::async_trait(?Send)]
impl LanguageBackend for Perl {
    async fn install(
        &self,
        store: &Store,
        hook: Arc<Hook>,
        install_cwd: &Path,
        reporter: &HookInstallReporter,
    ) -> Result<InstalledHook> {
        let progress = reporter.on_install_start(&hook);

        let mut info = InstallInfo::new(&hook, &store.hooks_dir())?;

        debug!(%hook, target = %info.env_path.display(), "Installing Perl environment");

        let cpan = which::which("cpan").context(
            "Failed to locate cpan executable. Is cpan installed and available in PATH?",
        )?;

        if hook.repo_path().is_some() {
            Cmd::new(&cpan)
                .current_dir(install_cwd)
                .arg("-T")
                .arg(".")
                .args(&hook.additional_dependencies)
                .envs(perl_env(&info.env_path)?)
                .check(true)
                .output()
                .await
                .context("Failed to install Perl dependencies")?;
        } else if !hook.additional_dependencies.is_empty() {
            Cmd::new(&cpan)
                .current_dir(install_cwd)
                .arg("-T")
                .args(&hook.additional_dependencies)
                .envs(perl_env(&info.env_path)?)
                .check(true)
                .output()
                .await
                .context("Failed to install Perl dependencies")?;
        }

        info.persist_env_path();

        reporter.on_install_complete(progress);

        Ok(InstalledHook::Installed {
            hook,
            info: Arc::new(info),
        })
    }

    async fn check_health(&self, _info: &InstallInfo) -> Result<()> {
        Ok(())
    }

    fn execution_environment(
        &self,
        _store: &Store,
        hook: &InstalledHook,
    ) -> Result<ExecutionEnvironment> {
        let env_dir = hook.env_path().expect("Perl must have env path");
        let new_path = prepend_paths(&[&bin_dir(env_dir)]).context("Failed to join PATH")?;

        let mut environment = ExecutionEnvironment::new();
        environment.set_path(&new_path).envs(perl_env(env_dir)?);
        Ok(environment)
    }
}

fn bin_dir(env_path: &Path) -> PathBuf {
    env_path.join("bin")
}

fn perl_env(env_path: &Path) -> Result<[(&'static str, OsString); 3]> {
    let env_path_str = env_path.to_string_lossy();
    let quoted_env_path = shlex::try_quote(&env_path_str)
        .context("Failed to quote Perl environment path")?
        .into_owned();

    Ok([
        (
            // PERL5LIB makes Perl load modules installed into this hook env at runtime.
            EnvVars::PERL5LIB,
            env_path.join("lib").join("perl5").into_os_string(),
        ),
        (
            // PERL_MB_OPT is consumed by Module::Build installers to install into this hook env.
            EnvVars::PERL_MB_OPT,
            format!("--install_base {quoted_env_path}").into(),
        ),
        (
            // PERL_MM_OPT is consumed by ExtUtils::MakeMaker installers to install into this hook env.
            EnvVars::PERL_MM_OPT,
            format!(
                "INSTALL_BASE={quoted_env_path} INSTALLSITEMAN1DIR=none INSTALLSITEMAN3DIR=none"
            )
            .into(),
        ),
    ])
}
