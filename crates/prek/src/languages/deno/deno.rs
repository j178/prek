use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use anyhow::{Context, Result};
use prek_consts::env_vars::EnvVars;
use prek_consts::prepend_paths;
use tracing::debug;

use crate::cli::reporter::{HookInstallReporter, HookRunReporter};
use crate::hook::{Hook, InstallInfo, InstalledHook};
use crate::languages::LanguageImpl;
use crate::languages::deno::DenoRequest;
use crate::languages::deno::installer::{DenoInstaller, DenoResult, bin_dir};
use crate::languages::version::LanguageRequest;
use crate::process::Cmd;
use crate::run::run_by_batch;
use crate::store::{CacheBucket, Store, ToolBucket};

/// Deno built-in subcommands that don't need "run" prefix.
const DENO_SUBCOMMANDS: &[&str] = &[
    "bench",
    "bundle",
    "cache",
    "check",
    "compile",
    "completions",
    "coverage",
    "doc",
    "eval",
    "fmt",
    "info",
    "init",
    "install",
    "jupyter",
    "lint",
    "lsp",
    "publish",
    "repl",
    "run",
    "serve",
    "task",
    "test",
    "types",
    "uninstall",
    "upgrade",
    "vendor",
];

/// Build the deno command from the entry parts.
///
/// Returns (command, args) where command is the deno binary path and args are
/// the arguments to pass to deno.
///
/// Logic:
/// - If entry starts with `deno`, use the rest as args (e.g., `deno fmt` -> args: `["fmt"]`)
/// - If entry starts with a deno subcommand, prepend nothing (e.g., `fmt` -> args: `["fmt"]`)
/// - Otherwise, assume it's a script and prepend `run` (e.g., `./script.ts` -> args: `["run", "./script.ts"]`)
fn build_deno_command(deno_binary: &Path, entry_parts: &[String]) -> (PathBuf, Vec<String>) {
    let first = entry_parts.first().map(String::as_str).unwrap_or("");

    // Entry starts with "deno" - use the rest as args
    if first == "deno" {
        return (deno_binary.to_path_buf(), entry_parts[1..].to_vec());
    }

    // Entry starts with a deno subcommand - use as-is
    if DENO_SUBCOMMANDS.contains(&first) {
        return (deno_binary.to_path_buf(), entry_parts.to_vec());
    }

    // Otherwise, prepend "run" for script execution
    let mut args = vec!["run".to_string()];
    args.extend(entry_parts.iter().cloned());
    (deno_binary.to_path_buf(), args)
}

/// Find the script in the entry that should be cached.
fn find_script_to_cache(entry: &[String]) -> Option<&str> {
    let first = entry.first()?.as_str();

    // Skip built-in Deno commands that don't need caching
    if matches!(
        first,
        "fmt" | "lint" | "test" | "check" | "bundle" | "doc" | "repl" | "eval"
    ) {
        return None;
    }

    // For "deno run ...", find the script after flags
    let candidates = if first == "run" { &entry[1..] } else { entry };

    candidates
        .iter()
        .map(String::as_str)
        .find(|s| !s.starts_with('-') && is_cacheable_script(s))
}

fn is_cacheable_script(script: &str) -> bool {
    if script.starts_with("http") || script.starts_with("jsr:") || script.starts_with("npm:") {
        return true;
    }

    std::path::Path::new(script).extension().is_some_and(|ext| {
        ext.eq_ignore_ascii_case("ts")
            || ext.eq_ignore_ascii_case("js")
            || ext.eq_ignore_ascii_case("mjs")
            || ext.eq_ignore_ascii_case("tsx")
            || ext.eq_ignore_ascii_case("jsx")
    })
}

#[derive(Debug, Copy, Clone)]
pub(crate) struct Deno;

impl LanguageImpl for Deno {
    async fn install(
        &self,
        hook: Arc<Hook>,
        store: &Store,
        reporter: &HookInstallReporter,
    ) -> Result<InstalledHook> {
        let progress = reporter.on_install_start(&hook);

        // 1. Install deno
        //   1) Find from `$PREK_HOME/tools/deno`
        //   2) Find from system
        //   3) Download from remote
        // 2. Create env
        // 3. Install dependencies

        // 1. Install deno
        let deno_dir = store.tools_path(ToolBucket::Deno);
        let installer = DenoInstaller::new(deno_dir);

        let (deno_request, allows_download) = match &hook.language_request {
            LanguageRequest::Any { system_only } => (&DenoRequest::Any, !system_only),
            LanguageRequest::Deno(deno_request) => (deno_request, true),
            _ => unreachable!(),
        };
        let deno = installer
            .install(store, deno_request, allows_download)
            .await
            .context("Failed to install deno")?;

        let mut info = InstallInfo::new(
            hook.language,
            hook.env_key_dependencies().clone(),
            &store.hooks_dir(),
        )?;

        info.with_toolchain(deno.deno().to_path_buf());
        // DenoVersion implements Deref<Target = semver::Version>, so we clone the inner version
        info.with_language_version((**deno.version()).clone());

        // 2. Create env
        let env_bin_dir = bin_dir(&info.env_path);
        fs_err::tokio::create_dir_all(&env_bin_dir).await?;

        // Create isolated DENO_DIR for this hook's cache
        let deno_cache_dir = store.cache_path(CacheBucket::Deno);
        fs_err::tokio::create_dir_all(&deno_cache_dir).await?;

        // `deno` needs to be in PATH for scripts that use `/usr/bin/env deno`
        let deno_bin_dir = deno.deno().parent().expect("Deno binary must have parent");
        let new_path =
            prepend_paths(&[&env_bin_dir, deno_bin_dir]).context("Failed to join PATH")?;

        // 3. Install dependencies
        if hook.repo_path().is_some() || !hook.additional_dependencies.is_empty() {
            let deno_json = info.env_path.join("deno.json");

            // Copy deno.json or deno.jsonc from repo if it exists
            if let Some(repo_path) = hook.repo_path() {
                // Try deno.json first, then deno.jsonc
                let repo_deno_json = repo_path.join("deno.json");
                let repo_deno_jsonc = repo_path.join("deno.jsonc");
                if repo_deno_json.exists() {
                    fs_err::tokio::copy(&repo_deno_json, &deno_json).await?;
                } else if repo_deno_jsonc.exists() {
                    // Copy jsonc as json - Deno can read it, and deno add will update it
                    fs_err::tokio::copy(&repo_deno_jsonc, &deno_json).await?;
                }
                // Also copy deno.lock if it exists
                let repo_lock = repo_path.join("deno.lock");
                if repo_lock.exists() {
                    fs_err::tokio::copy(&repo_lock, info.env_path.join("deno.lock")).await?;
                }
            }

            // Create minimal deno.json if none exists
            if !deno_json.exists() {
                fs_err::tokio::write(&deno_json, "{}").await?;
            }

            // Install additional dependencies via `deno add`
            if !hook.additional_dependencies.is_empty() {
                debug!(deps = ?hook.additional_dependencies, "Installing deno dependencies");
                Cmd::new(deno.deno(), "deno add")
                    .current_dir(&info.env_path)
                    .env(EnvVars::PATH, &new_path)
                    .env(EnvVars::DENO_DIR, &deno_cache_dir)
                    .arg("add")
                    .args(&hook.additional_dependencies)
                    .check(true)
                    .output()
                    .await
                    .context("Failed to install deno dependencies")?;
            }
        }

        // 4. Cache entry script dependencies
        if let Some(script) = find_script_to_cache(&hook.entry.split()?) {
            debug!(script, "Caching entry script dependencies");
            Cmd::new(deno.deno(), "deno cache")
                .current_dir(hook.work_dir())
                .env(EnvVars::PATH, &new_path)
                .env(EnvVars::DENO_DIR, &deno_cache_dir)
                .arg("cache")
                .arg(script)
                .check(true)
                .output()
                .await
                .context("Failed to cache entry script dependencies")?;
        }

        info.persist_env_path();

        reporter.on_install_complete(progress);

        Ok(InstalledHook::Installed {
            hook,
            info: Arc::new(info),
        })
    }

    async fn check_health(&self, info: &InstallInfo) -> Result<()> {
        let deno = DenoResult::from_executable(info.toolchain.clone())
            .fill_version()
            .await
            .context("Failed to query deno version")?;

        if **deno.version() != info.language_version {
            anyhow::bail!(
                "Deno version mismatch: expected {}, found {}",
                info.language_version,
                deno.version()
            );
        }

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

        let deno_cache_dir = store.cache_path(CacheBucket::Deno);
        let info = hook.install_info().expect("Deno must be installed");
        let deno_binary = &info.toolchain;
        let env_dir = &info.env_path;
        let deno_bin_dir = deno_binary.parent().expect("Deno binary must have parent");
        let new_path =
            prepend_paths(&[&bin_dir(env_dir), deno_bin_dir]).context("Failed to join PATH")?;

        // Split the entry and construct the deno command
        let entry_parts = hook.entry.split()?;
        let (cmd, args) = build_deno_command(deno_binary, &entry_parts);

        let run = async |batch: &[&Path]| {
            let mut output = Cmd::new(&cmd, "deno hook")
                .current_dir(hook.work_dir())
                .args(&args)
                .env(EnvVars::PATH, &new_path)
                .env(EnvVars::DENO_DIR, &deno_cache_dir)
                .envs(&hook.env)
                .args(&hook.args)
                .args(batch)
                .check(false)
                .stdin(Stdio::null())
                .pty_output()
                .await?;

            reporter.on_run_progress(progress, batch.len() as u64);

            output.stdout.extend(output.stderr);
            let code = output.status.code().unwrap_or(1);
            anyhow::Ok((code, output.stdout))
        };

        let full_entry: Vec<String> = std::iter::once(cmd.to_string_lossy().to_string())
            .chain(args.iter().cloned())
            .collect();
        let results = run_by_batch(hook, filenames, &full_entry, run).await?;

        reporter.on_run_complete(progress);

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
