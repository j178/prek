use serde::Deserialize;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::OnceCell;

use anyhow::{Context, Result};
use prek_consts::env_vars::EnvVars;
use prek_consts::prepend_paths;
use tracing::debug;

use crate::cli::reporter::{HookInstallReporter, HookRunReporter};
use crate::hook::{Hook, InstallInfo, InstalledHook};
use crate::languages::LanguageImpl;
use crate::process::Cmd;
use crate::run::run_by_batch;
use crate::store::Store;

static CABAL_UPDATE: OnceCell<()> = OnceCell::const_new();

pub(crate) const EXTRA_KEY_CABAL_VERSION: &str = "cabal_version";
pub(crate) const EXTRA_KEY_COMPILER_ID: &str = "compiler_id";

pub(crate) struct HaskellInfo {
    pub(crate) cabal_version: String,
    pub(crate) compiler_id: String,
    pub(crate) compiler_executable: std::path::PathBuf,
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case")]
struct CabalPathOutput {
    // The version of cabal
    // e.g. "3.6.2.0"
    cabal_version: String,
    compiler: CompilerInfo,
}

#[derive(Deserialize)]
struct CompilerInfo {
    // The version of the compiler
    // e.g. "ghc-9.2.4", "hugs-2006.09", "uhc-1.1.9.0"
    id: String,
    // The path to the compiler executable
    // e.g. "/usr/bin/ghc-9.2.4"
    path: std::path::PathBuf,
}

pub(crate) async fn query_haskell_info() -> Result<HaskellInfo> {
    let stdout = Cmd::new("cabal", "get haskell info")
        .arg("path")
        .arg("-z")
        .arg("--compiler-info")
        .arg("--output-format=json")
        .check(true)
        .output()
        .await
        .context("Failed to run cabal path")?
        .stdout;

    let output: CabalPathOutput =
        serde_json::from_slice(&stdout).context("Failed to parse cabal path JSON output")?;

    let cabal_version = output.cabal_version;
    let compiler_id = output.compiler.id;
    let compiler_executable = output.compiler.path;

    Ok(HaskellInfo {
        cabal_version,
        compiler_id,
        compiler_executable,
    })
}

#[derive(Debug, Copy, Clone)]
pub(crate) struct Haskell;

impl LanguageImpl for Haskell {
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

        debug!(%hook, target = %info.env_path.display(), "Installing Haskell environment");

        let haskell_info = query_haskell_info()
            .await
            .context("Failed to query Haskell info")?;

        let bindir = info.env_path.join("bin");
        std::fs::create_dir_all(&bindir).context("Failed to create bin directory")?;

        // Identify packages: *.cabal files in repo + additional_dependencies
        let mut pkgs = Vec::new();
        let search_path = hook.repo_path().unwrap_or(hook.project().path());
        if let Ok(entries) = std::fs::read_dir(search_path) {
            pkgs.extend(
                entries
                    .filter_map(Result::ok)
                    .map(|entry| entry.path())
                    .filter(|path| path.is_file())
                    .filter(|path| path.extension().is_some_and(|ext| ext == "cabal"))
                    .filter_map(|path| {
                        path.file_name()
                            .map(|name| name.to_string_lossy().to_string())
                    }),
            );
        }
        pkgs.extend(hook.additional_dependencies.clone());

        if pkgs.is_empty() {
            anyhow::bail!("Expected .cabal files or additional_dependencies");
        }

        // cabal update，execute once
        CABAL_UPDATE
            .get_or_try_init(|| async {
                Cmd::new("cabal", "update cabal package database")
                    .arg("update")
                    .check(true)
                    .output()
                    .await
                    .context("Failed to run `cabal update`")
                    .map(|_| ())
            })
            .await?;

        // cabal install --install-method copy --installdir <bindir> <pkgs>
        Cmd::new("cabal", "install haskell dependencies")
            .current_dir(search_path)
            .arg("install")
            .arg("--install-method")
            .arg("copy")
            .arg("--installdir")
            .arg(&bindir)
            .args(pkgs)
            .check(true)
            .output()
            .await
            .context("Failed to install haskell dependencies")?;

        info.with_toolchain(haskell_info.compiler_executable)
            .with_extra(EXTRA_KEY_CABAL_VERSION, &haskell_info.cabal_version)
            .with_extra(EXTRA_KEY_COMPILER_ID, &haskell_info.compiler_id);

        info.persist_env_path();

        reporter.on_install_complete(progress);

        Ok(InstalledHook::Installed {
            hook,
            info: Arc::new(info),
        })
    }

    async fn check_health(&self, info: &InstallInfo) -> Result<()> {
        let current_haskell_info = query_haskell_info()
            .await
            .context("Failed to query haskell info")?;

        if current_haskell_info.compiler_executable != info.toolchain {
            anyhow::bail!(
                "Haskell executable mismatch: expected `{}`, found `{}`",
                info.toolchain.display(),
                current_haskell_info.compiler_executable.display()
            );
        }

        if let Some(expected_cabal_version) = info.get_extra(EXTRA_KEY_CABAL_VERSION) {
            if current_haskell_info.cabal_version != *expected_cabal_version {
                anyhow::bail!(
                    "Haskell cabal version mismatch: expected `{}`, found `{}`",
                    expected_cabal_version,
                    current_haskell_info.cabal_version
                );
            }
        }

        if let Some(expected_compiler_id) = info.get_extra(EXTRA_KEY_COMPILER_ID) {
            if current_haskell_info.compiler_id != *expected_compiler_id {
                anyhow::bail!(
                    "Haskell compiler id mismatch: expected `{}`, found `{}`",
                    expected_compiler_id,
                    current_haskell_info.compiler_id
                );
            }
        }

        Ok(())
    }

    async fn run(
        &self,
        hook: &InstalledHook,
        filenames: &[&Path],
        _store: &Store,
        reporter: &HookRunReporter,
    ) -> Result<(i32, Vec<u8>)> {
        let progress = reporter.on_run_start(hook, filenames.len());

        let env_dir = hook.env_path().expect("Haskell must have env path");
        let bindir = env_dir.join("bin");
        let new_path = prepend_paths(&[&bindir]).context("Failed to join PATH")?;

        let entry = hook.entry.resolve(Some(&new_path))?;

        let run = async |batch: &[&Path]| {
            let mut output = Cmd::new(&entry[0], "run haskell hook")
                .current_dir(hook.work_dir())
                .args(&entry[1..])
                .env(EnvVars::PATH, &new_path)
                .args(&hook.args)
                .args(batch)
                .check(false)
                .pty_output()
                .await?;

            reporter.on_run_progress(progress, batch.len() as u64);

            output.stdout.extend(output.stderr);
            let code = output.status.code().unwrap_or(1);
            anyhow::Ok((code, output.stdout))
        };

        let results = run_by_batch(hook, filenames, &entry, run).await?;

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
