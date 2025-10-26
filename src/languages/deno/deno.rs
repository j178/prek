use std::env::consts::EXE_EXTENSION;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use constants::env_vars::EnvVars;

use crate::cli::reporter::HookInstallReporter;
use crate::hook::InstalledHook;
use crate::hook::{Hook, InstallInfo};
use crate::languages::LanguageImpl;
use crate::languages::deno::DenoRequest;
use crate::languages::deno::installer::{DenoInstaller, DenoResult, bin_dir};
use crate::languages::version::LanguageRequest;
use crate::process::Cmd;
use crate::run::{prepend_paths, run_by_batch};
use crate::store::{CacheBucket, Store, ToolBucket};

/// Find the script in the entry that should be cached.
/// Handles both direct scripts and `deno run ...` commands.
fn find_script_to_cache(entry: &[String]) -> Option<&str> {
    if entry.is_empty() {
        return None;
    }

    // Check if first arg is a built-in Deno command
    let first = entry[0].as_str();
    if matches!(
        first,
        "fmt" | "lint" | "test" | "check" | "bundle" | "doc" | "repl" | "eval"
    ) {
        return None;
    }

    // If first arg is "run", look for the script after flags
    if first == "run" {
        for arg in &entry[1..] {
            let arg_str = arg.as_str();
            // Skip flags
            if arg_str.starts_with('-') {
                continue;
            }
            // Found the script
            if is_cacheable_script(arg_str) {
                return Some(arg_str);
            }
        }
        return None;
    }

    // Otherwise, check if the first arg itself is a script
    if is_cacheable_script(first) {
        Some(first)
    } else {
        None
    }
}

/// Check if a script path should be cached.
fn is_cacheable_script(script: &str) -> bool {
    script.starts_with("http")
        || script.starts_with("jsr:")
        || script.starts_with("npm:")
        || script.contains('/')
        || script.ends_with(".ts")
        || script.ends_with(".js")
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
        //   3) Download from GitHub releases
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

        // Create env for isolated dependencies
        let mut info = InstallInfo::new(
            hook.language,
            hook.dependencies().clone(),
            &store.hooks_dir(),
        )?;

        info.with_toolchain(deno.deno().to_path_buf());
        info.with_language_version((**deno.version()).clone());

        // 2. Create env with bin directory
        let bin_dir_path = bin_dir(&info.env_path);
        fs_err::tokio::create_dir_all(&bin_dir_path).await?;

        // Create symlink to deno executable in the bin directory
        crate::fs::create_symlink_or_copy(
            deno.deno(),
            &bin_dir_path.join("deno").with_extension(EXE_EXTENSION),
        )
        .await?;

        let deno_cache_dir = store.cache_path(CacheBucket::Deno);
        fs_err::tokio::create_dir_all(&deno_cache_dir).await?;

        // Initialize deno.json if we have dependencies to install
        if hook.repo_path().is_some() || !hook.additional_dependencies.is_empty() {
            let deno_json = info.env_path.join("deno.json");

            // Check if repo has deno.json to copy, otherwise create minimal one
            let mut needs_deno_json = true;
            if let Some(repo_path) = hook.repo_path() {
                let repo_deno_json = repo_path.join("deno.json");
                if repo_deno_json.exists() {
                    // Copy the deno.json from the repo
                    fs_err::tokio::copy(repo_deno_json, &deno_json).await?;
                    needs_deno_json = false;
                }
            }

            if needs_deno_json {
                // Create a minimal deno.json for dependency management
                fs_err::tokio::write(&deno_json, "{}").await?;
            }

            // Install additional dependencies
            // Prepend bin_dir to PATH so deno can be found
            let new_path = prepend_paths(&[&bin_dir_path]).context("Failed to join PATH")?;

            if !hook.additional_dependencies.is_empty() {
                Cmd::new(
                    &bin_dir_path.join("deno").with_extension(EXE_EXTENSION),
                    "deno add",
                )
                .current_dir(&info.env_path)
                .env(EnvVars::PATH, &new_path)
                .env(EnvVars::DENO_DIR, &deno_cache_dir)
                .arg("add")
                .args(&hook.additional_dependencies)
                .check(true)
                .output()
                .await?;
            }
        }

        // Cache the entry script and its dependencies for offline use
        let entry = hook.entry.split()?;
        if let Some(script) = find_script_to_cache(&entry) {
            let work_dir = hook.work_dir();
            let new_path = prepend_paths(&[&bin_dir_path]).context("Failed to join PATH")?;

            Cmd::new(
                &bin_dir_path.join("deno").with_extension(EXE_EXTENSION),
                "deno cache",
            )
            .current_dir(work_dir)
            .env(EnvVars::PATH, &new_path)
            .env(EnvVars::DENO_DIR, &deno_cache_dir)
            .arg("cache")
            .arg(script)
            .check(true)
            .output()
            .await
            .context("Failed to cache entry script dependencies")?;
        }

        reporter.on_install_complete(progress);

        Ok(InstalledHook::Installed {
            hook,
            info: Arc::new(info),
        })
    }

    async fn check_health(&self, info: &InstallInfo) -> Result<()> {
        let current = DenoResult::from_executable(info.toolchain.clone())
            .fill_version()
            .await
            .context("Failed to query current Deno info")?;

        if **current.version() != info.language_version {
            anyhow::bail!(
                "Deno version mismatch: expected `{}`, found `{}`",
                info.language_version,
                current.version()
            );
        }
        if current.deno() != info.toolchain {
            anyhow::bail!(
                "Deno executable mismatch: expected `{}`, found `{}`",
                info.toolchain.display(),
                current.deno().display()
            );
        }

        Ok(())
    }

    async fn run(
        &self,
        hook: &InstalledHook,
        filenames: &[&Path],
        store: &Store,
    ) -> Result<(i32, Vec<u8>)> {
        let deno_dir = store.cache_path(CacheBucket::Deno);
        let env_dir = hook.env_path().expect("Deno must have env path");

        // Prepend bin directory to PATH so scripts can find deno
        let new_path = prepend_paths(&[&bin_dir(env_dir)]).context("Failed to join PATH")?;

        let entry = hook.entry.resolve(Some(&new_path))?;
        let run = async move |batch: &[&Path]| {
            let mut output = Cmd::new(&entry[0], "deno hook")
                .current_dir(hook.work_dir())
                .args(&entry[1..])
                .env(EnvVars::PATH, &new_path)
                .env(EnvVars::DENO_DIR, &deno_dir)
                .args(&hook.args)
                .args(batch)
                .check(false)
                .pty_output()
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
