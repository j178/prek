use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use prek_consts::env_vars::EnvVars;
use semver::Version;
use tracing::debug;

use crate::cli::reporter::HookInstallReporter;
use crate::hook::{Hook, InstallInfo, InstalledHook};
use crate::languages::LanguageImpl;
use crate::process::Cmd;
use crate::run::{prepend_paths, run_by_batch};
use crate::store::Store;

#[derive(Debug, Copy, Clone)]
pub(crate) struct Dart;

pub(crate) struct DartInfo {
    pub(crate) version: Version,
    pub(crate) executable: PathBuf,
}

pub(crate) async fn query_dart_info() -> Result<DartInfo> {
    let stdout = Cmd::new("dart", "get dart version")
        .arg("--version")
        .check(true)
        .output()
        .await?
        .stdout;

    // Parse output like "Dart SDK version: 3.0.0 (stable)"
    let version_str = String::from_utf8_lossy(&stdout);
    let version = version_str
        .split_whitespace()
        .nth(3)
        .context("Failed to get Dart version")?
        .trim();

    let version = Version::parse(version).context("Failed to parse Dart version")?;

    // Get the dart executable path
    let stdout = Cmd::new("which", "get dart executable")
        .arg("dart")
        .check(true)
        .output()
        .await?
        .stdout;

    let executable = PathBuf::from(String::from_utf8_lossy(&stdout).trim());

    Ok(DartInfo {
        version,
        executable,
    })
}

impl LanguageImpl for Dart {
    async fn install(
        &self,
        hook: Arc<Hook>,
        store: &Store,
        reporter: &HookInstallReporter,
    ) -> Result<InstalledHook> {
        let progress = reporter.on_install_start(&hook);

        let mut info = InstallInfo::new(
            hook.language,
            hook.dependencies().clone(),
            &store.hooks_dir(),
        )?;

        debug!(%hook, target = %info.env_path.display(), "Installing Dart environment");

        // Check dart is installed.
        let dart_info = query_dart_info().await.context("Failed to query Dart info")?;

        // Install dependencies for the remote repository.
        if let Some(repo_path) = hook.repo_path() {
            if Self::has_pubspec(repo_path) {
                Self::install_from_pubspec(&info.env_path, repo_path).await?;
            }
        }

        // Install additional dependencies.
        for dep in &hook.additional_dependencies {
            Self::install_dependency(&info.env_path, dep).await?;
        }

        info.with_toolchain(dart_info.executable)
            .with_language_version(dart_info.version);

        info.persist_env_path();

        reporter.on_install_complete(progress);

        Ok(InstalledHook::Installed {
            hook,
            info: Arc::new(info),
        })
    }

    async fn check_health(&self, info: &InstallInfo) -> Result<()> {
        let current_dart_info = query_dart_info()
            .await
            .context("Failed to query current Dart info")?;

        if current_dart_info.version != info.language_version {
            anyhow::bail!(
                "Dart version mismatch: expected `{}`, found `{}`",
                info.language_version,
                current_dart_info.version
            );
        }

        if current_dart_info.executable != info.toolchain {
            anyhow::bail!(
                "Dart executable mismatch: expected `{}`, found `{}`",
                info.toolchain.display(),
                current_dart_info.executable.display()
            );
        }

        Ok(())
    }

    async fn run(
        &self,
        hook: &InstalledHook,
        filenames: &[&Path],
        _store: &Store,
    ) -> Result<(i32, Vec<u8>)> {
        let env_dir = hook.env_path().expect("Dart must have env path");
        let new_path = prepend_paths(&[&env_dir.join("bin")]).context("Failed to join PATH")?;
        let entry = hook.entry.resolve(Some(&new_path))?;

        let pub_cache = env_dir.to_string_lossy().to_string();

        let run = async |batch: &[&Path]| {
            let mut output = Cmd::new(&entry[0], "run dart command")
                .current_dir(hook.work_dir())
                .args(&entry[1..])
                .env(EnvVars::PATH, &new_path)
                .env(EnvVars::PUB_CACHE, &pub_cache)
                .args(&hook.args)
                .args(batch)
                .check(false)
                .pty_output()
                .await?;

            output.stdout.extend(output.stderr);
            let code = output.status.code().unwrap_or(1);
            anyhow::Ok((code, output.stdout))
        };

        let results = run_by_batch(hook, filenames, &entry, run).await?;

        let mut combined_status = 0;
        let mut combined_output = Vec::new();

        for (code, output) in results {
            combined_status |= code;
            combined_output.extend(output);
        }

        Ok((combined_status, combined_output))
    }
}

impl Dart {
    async fn install_from_pubspec(env_path: &Path, repo_path: &Path) -> Result<()> {
        // Run `dart pub get` to install dependencies from pubspec.yaml
        Cmd::new("dart", "dart pub get")
            .current_dir(repo_path)
            .env(EnvVars::PUB_CACHE, env_path.to_string_lossy().as_ref())
            .arg("pub")
            .arg("get")
            .check(true)
            .output()
            .await
            .context("Failed to run dart pub get")?;

        Ok(())
    }

    async fn install_dependency(env_path: &Path, dependency: &str) -> Result<()> {
        // Parse dependency - format is "package" or "package:version"
        let (package, version) = if let Some((pkg, ver)) = dependency.split_once(':') {
            (pkg, Some(ver))
        } else {
            (dependency, None)
        };

        // Use `dart pub cache add` to add the dependency
        let mut cmd = Cmd::new("dart", "dart pub cache add");
        cmd.env(EnvVars::PUB_CACHE, env_path.to_string_lossy().as_ref())
            .arg("pub")
            .arg("cache")
            .arg("add")
            .arg(package);

        if let Some(ver) = version {
            cmd.arg("--version").arg(ver);
        }

        cmd.check(true)
            .output()
            .await
            .context("Failed to install Dart dependency")?;

        Ok(())
    }

    fn has_pubspec(repo_path: &Path) -> bool {
        repo_path.join("pubspec.yaml").exists()
    }
}
