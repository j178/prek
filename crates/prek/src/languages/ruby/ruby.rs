use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use anyhow::{Context, Result};
use prek_consts::env_vars::EnvVars;
use tracing::debug;

use crate::cli::reporter::HookInstallReporter;
use crate::hook::{Hook, InstallInfo, InstalledHook};
use crate::languages::LanguageImpl;
use crate::languages::ruby::RubyRequest;
use crate::languages::ruby::gem::{build_gemspecs, install_gems};
use crate::languages::ruby::installer::RubyInstaller;
use crate::languages::version::LanguageRequest;
use crate::process::Cmd;
use crate::run::{prepend_paths, run_by_batch};
use crate::store::Store;

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
        let installer = RubyInstaller::new();

        let request = match &hook.language_request {
            LanguageRequest::Any { system_only: _ } => &RubyRequest::Any,
            LanguageRequest::Ruby(req) => req,
            _ => unreachable!(),
        };

        let ruby = installer
            .install(store, request)
            .await
            .context("Failed to install Ruby")?;

        // 2. Create InstallInfo
        let mut info = InstallInfo::new(
            hook.language,
            hook.dependencies().clone(),
            &store.hooks_dir(),
        )?;

        info.with_toolchain(ruby.ruby_bin().to_path_buf())
            .with_language_version(ruby.version().clone());

        // Store Ruby engine in metadata
        info.with_extra("ruby_engine", ruby.engine());

        // 3. Create environment directories
        let gem_home = gem_home(&info.env_path);
        fs_err::tokio::create_dir_all(&gem_home).await?;
        fs_err::tokio::create_dir_all(gem_home.join("bin")).await?;

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

        info.persist_env_path();

        reporter.on_install_complete(progress);

        Ok(InstalledHook::Installed {
            hook,
            info: Arc::new(info),
        })
    }

    async fn check_health(&self, info: &InstallInfo) -> Result<()> {
        // 1. Verify Ruby executable exists
        if !info.toolchain.exists() {
            anyhow::bail!("Ruby executable not found at {}", info.toolchain.display());
        }

        // 2. Verify it runs and reports correct version
        let script = "puts RUBY_VERSION";
        let output = Cmd::new(&info.toolchain, "check ruby version")
            .arg("-e")
            .arg(script)
            .check(true)
            .output()
            .await?;

        let version_str = str::from_utf8(&output.stdout)?.trim();
        let actual_version = semver::Version::parse(version_str)
            .with_context(|| format!("Failed to parse Ruby version: {version_str}"))?;

        if actual_version != info.language_version {
            anyhow::bail!(
                "Ruby version mismatch: expected {}, found {}",
                info.language_version,
                actual_version
            );
        }

        // 3. Verify gem home exists
        let gem_home = gem_home(&info.env_path);
        if !gem_home.exists() {
            anyhow::bail!("Gem home directory not found at {}", gem_home.display());
        }

        // 4. Verify gem bin directory exists
        let gem_bin = gem_home.join("bin");
        if !gem_bin.exists() {
            anyhow::bail!("Gem bin directory not found at {}", gem_bin.display());
        }

        Ok(())
    }

    async fn run(
        &self,
        hook: &InstalledHook,
        filenames: &[&Path],
        _store: &Store,
    ) -> Result<(i32, Vec<u8>)> {
        let env_dir = hook.env_path().expect("Ruby hook must have env path");
        let info = hook.install_info().expect("Ruby hook must be installed");

        // Prepare PATH
        let gem_home = gem_home(env_dir);
        let gem_bin = gem_home.join("bin");
        let ruby_bin = info
            .toolchain
            .parent()
            .expect("Ruby toolchain should have parent");

        let new_path = prepend_paths(&[&gem_bin, ruby_bin]).context("Failed to join PATH")?;

        // Resolve entry point
        let entry = hook.entry.resolve(Some(&new_path))?;

        // Execute in batches
        let run = async |batch: &[&Path]| {
            let mut output = Cmd::new(&entry[0], "ruby hook")
                .current_dir(hook.work_dir())
                .env(EnvVars::PATH, &new_path)
                .env(EnvVars::GEM_HOME, &gem_home)
                .env(EnvVars::BUNDLE_IGNORE_CONFIG, "1")
                .env_remove(EnvVars::GEM_PATH)
                .env_remove(EnvVars::BUNDLE_GEMFILE)
                .args(&entry[1..])
                .args(&hook.args)
                .args(batch)
                .check(false)
                .stdin(Stdio::null())
                .pty_output()
                .await?;

            output.stdout.extend(output.stderr);
            let code = output.status.code().unwrap_or(1);
            anyhow::Ok((code, output.stdout))
        };

        let results = run_by_batch(hook, filenames, &entry, run).await?;

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
