use std::fmt::Write as _;
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
    let executable = which::which("dart")
        .context("Failed to locate dart executable. Is Dart installed and available in PATH?")?;
    debug!("Found dart executable at: {}", executable.display());

    // Use the executable path we found, not just "dart"
    let stdout = Cmd::new(&executable, "get dart version")
        .arg("--version")
        .check(true)
        .output()
        .await?
        .stdout;

    // Parse output like "Dart SDK version: 3.0.0 (stable)"
    // Handle Flutter SDK which may output extra lines before the version
    let version = str::from_utf8(&stdout)
        .context("Failed to parse `dart --version` output as UTF-8")?
        .lines()
        .find(|line| line.contains("Dart SDK version:"))
        .and_then(|line| line.split_whitespace().nth(3))
        .context("Failed to extract Dart version from output")?
        .trim();

    let version = Version::parse(version).context("Failed to parse Dart version")?;

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
        let dart_info = query_dart_info()
            .await
            .context("Failed to query Dart info")?;

        // Install dependencies for the remote repository.
        if let Some(repo_path) = hook.repo_path() {
            if Self::has_pubspec(repo_path) {
                Self::install_from_pubspec(&dart_info.executable, &info.env_path, repo_path)
                    .await?;
            }
        }

        // Install additional dependencies by creating a pubspec.yaml
        if !hook.additional_dependencies.is_empty() {
            Self::install_additional_dependencies(
                &dart_info.executable,
                &info.env_path,
                &hook.additional_dependencies,
            )
            .await?;
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

        Self::setup_package_config(env_dir, hook.work_dir())?;

        let run = async |batch: &[&Path]| {
            let mut output = Cmd::new(&entry[0], "run dart command")
                .current_dir(hook.work_dir())
                .args(&entry[1..])
                .env(EnvVars::PATH, &new_path)
                .env(EnvVars::PUB_CACHE, env_dir)
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
    fn setup_package_config(env_dir: &Path, work_dir: &Path) -> Result<()> {
        let env_package_config = env_dir.join(".dart_tool").join("package_config.json");
        if env_package_config.exists() {
            let work_dart_tool = work_dir.join(".dart_tool");
            fs_err::create_dir_all(&work_dart_tool)
                .context("Failed to create .dart_tool directory in work_dir")?;
            let work_package_config = work_dart_tool.join("package_config.json");
            fs_err::copy(&env_package_config, &work_package_config)
                .context("Failed to copy package_config.json to work_dir")?;
        }
        Ok(())
    }

    async fn install_from_pubspec(dart: &Path, env_path: &Path, repo_path: &Path) -> Result<()> {
        // Run `dart pub get` to install dependencies from pubspec.yaml
        Cmd::new(dart, "dart pub get")
            .current_dir(repo_path)
            .env(EnvVars::PUB_CACHE, env_path)
            .arg("pub")
            .arg("get")
            .check(true)
            .output()
            .await
            .context("Failed to run dart pub get")?;

        Ok(())
    }

    async fn install_additional_dependencies(
        dart: &Path,
        env_path: &Path,
        dependencies: &rustc_hash::FxHashSet<String>,
    ) -> Result<()> {
        // Create a minimal pubspec.yaml with the additional dependencies
        let mut pubspec_content = indoc::formatdoc! {"
            name: prek_dart_env
            environment:
              sdk: '>=2.12.0 <4.0.0'
            dependencies:
        "};

        for dep in dependencies {
            // Parse dependency - format is "package" or "package:version"
            if let Some((package, version)) = dep.split_once(':') {
                writeln!(pubspec_content, "  {package}: {version}")?;
            } else {
                writeln!(pubspec_content, "  {dep}: any")?;
            }
        }

        // Write pubspec.yaml to env_path
        let pubspec_path = env_path.join("pubspec.yaml");
        fs_err::tokio::write(&pubspec_path, pubspec_content).await?;

        // Run `dart pub get` to resolve and install dependencies
        Cmd::new(dart, "dart pub get")
            .current_dir(env_path)
            .env(EnvVars::PUB_CACHE, env_path)
            .arg("pub")
            .arg("get")
            .check(true)
            .output()
            .await
            .context("Failed to run dart pub get for additional dependencies")?;

        Ok(())
    }

    fn has_pubspec(repo_path: &Path) -> bool {
        repo_path.join("pubspec.yaml").exists()
    }
}
