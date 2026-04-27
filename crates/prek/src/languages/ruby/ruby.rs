use std::env::consts::EXE_EXTENSION;
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
use crate::languages::ruby::RubyRequest;
use crate::languages::ruby::gem::{build_gemspecs, install_gems};
use crate::languages::ruby::installer::{RubyInstaller, query_ruby_info};
use crate::languages::version::LanguageRequest;
use crate::process::Cmd;
use crate::run::run_by_batch;
use crate::store::{Store, ToolBucket};

#[derive(Debug, Copy, Clone)]
pub(crate) struct Ruby;

impl LanguageImpl for Ruby {
    async fn install(
        &self,
        hook: Arc<Hook>,
        store: &Store,
        reporter: &HookInstallReporter,
    ) -> Result<InstalledHook> {
        let progress = reporter.on_install_start(&hook);

        // 1. Install Ruby
        let ruby_dir = store.tools_path(ToolBucket::Ruby);
        let installer = RubyInstaller::new(ruby_dir);

        let (request, allows_download) = match &hook.language_request {
            LanguageRequest::Any { system_only } => (&RubyRequest::Any, !system_only),
            LanguageRequest::Ruby(req) => (req, true),
            _ => unreachable!(),
        };

        let ruby = installer
            .install(store, request, allows_download)
            .await
            .context("Failed to install Ruby")?;

        // 2. Create InstallInfo
        let mut info = InstallInfo::new(
            hook.language,
            hook.env_key_dependencies().clone(),
            &store.hooks_dir(),
        )?;

        info.with_toolchain(ruby.ruby_bin().to_path_buf())
            .with_language_version(ruby.version().clone());

        // Store Ruby engine in metadata
        info.with_extra("ruby_engine", ruby.engine());

        // 3. Create environment directories
        let gem_home = gem_home(&info.env_path);
        fs_err::tokio::create_dir_all(&gem_home).await?;
        let gem_bin = gem_bin(&info.env_path);
        fs_err::tokio::create_dir_all(&gem_bin).await?;

        // 4. Build gemspecs
        if let Some(repo_path) = hook.repo_path() {
            // Try to build gemspecs, but don't fail if there aren't any
            match build_gemspecs(&ruby, repo_path).await {
                Ok(gem_files) => {
                    debug!("Built {} gem(s) from gemspecs", gem_files.len());
                }
                Err(e) if e.to_string().contains("No .gemspec files") => {
                    debug!("No gemspecs found in repo, skipping gem build");
                }
                Err(e) => return Err(e).context("Failed to build gemspecs"),
            }
        }

        // 5. Install gems (Note that pre-commit installs all *.gem files, not only those built from gemspecs)
        install_gems(
            &ruby,
            &gem_home,
            hook.repo_path(),
            &hook.additional_dependencies,
        )
        .await
        .context("Failed to install gems")?;

        let gem_bin_ruby = gem_bin.join("ruby").with_extension(EXE_EXTENSION);
        crate::fs::symlink_or_copy(ruby.ruby_bin(), &gem_bin_ruby)
            .await
            .context("Failed to install Ruby executable into gem bin directory")?;

        info.persist_env_path();

        reporter.on_install_complete(progress);

        Ok(InstalledHook::Installed {
            hook,
            info: Arc::new(info),
        })
    }

    async fn check_health(&self, info: &InstallInfo) -> Result<()> {
        // 1. Verify Ruby runs and reports correct version
        let (actual_version, _) = query_ruby_info(&info.toolchain)
            .await
            .context("Failed to query Ruby info")?;

        if actual_version != info.language_version {
            anyhow::bail!(
                "Ruby version mismatch: expected {}, found {}",
                info.language_version,
                actual_version
            );
        }

        // 2. Verify gem bin Ruby executable exists
        let gem_bin = gem_bin(&info.env_path);
        let gem_bin_ruby = gem_bin.join("ruby").with_extension(EXE_EXTENSION);
        if !gem_bin_ruby.exists() {
            anyhow::bail!(
                "Gem bin Ruby executable not found at {}",
                gem_bin_ruby.display()
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

        let env_dir = hook.env_path().expect("Ruby hook must have env path");

        // Prepare PATH
        let gem_home = gem_home(env_dir);
        let gem_bin = gem_bin(env_dir);
        let ruby_bin = hook
            .toolchain_dir()
            .expect("Ruby toolchain should have parent");

        let new_path = prepend_paths(&[&gem_bin, ruby_bin]).context("Failed to join PATH")?;

        // Resolve entry point
        let entry = hook.entry.resolve(Some(&new_path), store)?;

        // Execute in batches
        let run = async |batch: &[&Path]| {
            let mut output = Cmd::new(&entry[0], "ruby hook")
                .current_dir(hook.work_dir())
                .env(EnvVars::PATH, &new_path)
                .env(EnvVars::GEM_HOME, &gem_home)
                .env(EnvVars::BUNDLE_IGNORE_CONFIG, "1")
                .env_remove(EnvVars::GEM_PATH)
                .env_remove(EnvVars::BUNDLE_GEMFILE)
                .envs(&hook.env)
                .args(&entry[1..])
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

        let results = run_by_batch(hook, filenames, entry.argv(), run).await?;

        reporter.on_run_complete(progress);

        // Combine results
        let mut combined_status = 0;
        let mut combined_output = Vec::new();

        for (code, output) in results {
            combined_status |= code;
            combined_output.extend(output);
        }

        Ok((combined_status, combined_output))
    }
}

/// Get the `GEM_HOME` path for this environment
fn gem_home(env_path: &Path) -> PathBuf {
    env_path.join("gems")
}

fn gem_bin(env_path: &Path) -> PathBuf {
    gem_home(env_path).join("bin")
}
