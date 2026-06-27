use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use anyhow::{Context, Result};
use prek_consts::env_vars::EnvVars;
use prek_consts::prepend_paths;
use tracing::debug;

use crate::cli::reporter::HookInstallReporter;
use crate::cli::run::HookRunReporter;
use crate::hook::{Hook, InstallInfo, InstalledHook};
use crate::languages::LanguageImpl;
use crate::process::Cmd;
use crate::run::run_by_batch;
use crate::store::Store;

#[derive(Debug, Copy, Clone)]
pub(crate) struct Perl;

impl LanguageImpl for Perl {
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

        debug!(%hook, target = %info.env_path.display(), "Installing Perl environment");

        let cpan = which::which("cpan").context(
            "Failed to locate cpan executable. Is cpan installed and available in PATH?",
        )?;

        if let Some(repo_path) = hook.repo_path() {
            Cmd::new(&cpan)
                .current_dir(repo_path)
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

    async fn run(
        &self,
        hook: &InstalledHook,
        filenames: &[&Path],
        store: &Store,
        reporter: &HookRunReporter,
    ) -> Result<(i32, Vec<u8>)> {
        let progress = reporter.on_run_start(hook, filenames.len());

        let env_dir = hook.env_path().expect("Perl must have env path");
        let new_path = prepend_paths(&[&bin_dir(env_dir)]).context("Failed to join PATH")?;
        let entry = hook.entry.resolve(Some(&new_path), store)?;

        let run = async |batch: &[&Path]| {
            let mut output = Cmd::new(&entry[0])
                .current_dir(hook.work_dir())
                .args(&entry[1..])
                .env(EnvVars::PATH, &new_path)
                .envs(perl_env(env_dir)?)
                .envs(&hook.env)
                .args(&hook.args)
                .file_args(batch)
                .check(false)
                .stdin(Stdio::null())
                .pty_output_with_sink(reporter.output_sink(progress))
                .await?;

            reporter.on_run_progress(progress, batch.len() as u64);

            output.stdout.extend(output.stderr);
            let code = output.status.code().unwrap_or(1);
            anyhow::Ok((code, output.stdout))
        };

        let results = run_by_batch(hook, filenames, entry.argv(), run).await?;

        let mut combined_status = 0;
        let mut combined_output = Vec::new();

        for (code, output) in results {
            combined_status |= code;
            combined_output.extend(output);
        }

        reporter.on_run_complete(progress);

        Ok((combined_status, combined_output))
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
