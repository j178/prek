use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::str;
use std::sync::Arc;

use anyhow::{Context, Result};
use prek_consts::env_vars::EnvVars;
use prek_consts::prepend_paths;
use semver::Version;
use tracing::debug;

use crate::cli::reporter::{HookInstallReporter, HookRunReporter};
use crate::hook::{Hook, InstallInfo, InstalledHook};
use crate::languages::LanguageImpl;
use crate::process::Cmd;
use crate::run::run_by_batch;
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

    let stdout = Cmd::new(&executable, "get dart version")
        .arg("--version")
        .check(true)
        .output()
        .await?
        .stdout;

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
            hook.env_key_dependencies().clone(),
            &store.hooks_dir(),
        )?;

        debug!(%hook, target = %info.env_path.display(), "Installing Dart environment");

        let dart_info = query_dart_info()
            .await
            .context("Failed to query Dart info")?;

        let pubspec_source = if let Some(repo_path) = hook.repo_path() {
            Self::has_pubspec(repo_path).then_some(repo_path)
        } else {
            Self::has_pubspec(hook.work_dir()).then_some(hook.work_dir())
        };

        if let Some(source_path) = pubspec_source {
            Self::install_from_pubspec(&dart_info.executable, &info.env_path, source_path).await?;
        }

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
        reporter: &HookRunReporter,
    ) -> Result<(i32, Vec<u8>)> {
        let progress = reporter.on_run_start(hook, filenames.len());

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

    async fn compile_executables(
        dart: &Path,
        pubspec_path: &Path,
        bin_src_dir: &Path,
        bin_out_dir: &Path,
        pub_cache: &Path,
    ) -> Result<()> {
        let pubspec_content =
            fs_err::read_to_string(pubspec_path).context("Failed to read pubspec.yaml")?;
        let pubspec: serde_json::Value =
            serde_saphyr::from_str(&pubspec_content).context("Failed to parse pubspec.yaml")?;

        let Some(executables) = pubspec
            .get("executables")
            .and_then(serde_json::Value::as_object)
        else {
            return Ok(());
        };

        fs_err::create_dir_all(bin_out_dir).context("Failed to create bin output directory")?;

        for exe_name in executables.keys() {
            let source_file = bin_src_dir.join(format!("{exe_name}.dart"));
            if !source_file.exists() {
                debug!("Skipping executable '{exe_name}': source file not found");
                continue;
            }

            let output_path = if cfg!(windows) {
                bin_out_dir.join(format!("{exe_name}.exe"))
            } else {
                bin_out_dir.join(exe_name)
            };

            debug!(
                "Compiling executable '{exe_name}': {} -> {}",
                source_file.display(),
                output_path.display()
            );

            Cmd::new(dart, "dart compile exe")
                .arg("compile")
                .arg("exe")
                .arg(&source_file)
                .arg("--output")
                .arg(&output_path)
                .env(EnvVars::PUB_CACHE, pub_cache)
                .check(true)
                .output()
                .await
                .with_context(|| format!("Failed to compile executable '{exe_name}'"))?;
        }

        Ok(())
    }

    async fn install_from_pubspec(dart: &Path, env_path: &Path, repo_path: &Path) -> Result<()> {
        Cmd::new(dart, "dart pub get")
            .current_dir(repo_path)
            .env(EnvVars::PUB_CACHE, env_path)
            .arg("pub")
            .arg("get")
            .check(true)
            .output()
            .await
            .context("Failed to run dart pub get")?;

        let source_package_config = repo_path.join(".dart_tool").join("package_config.json");
        if source_package_config.exists() {
            let env_dart_tool = env_path.join(".dart_tool");
            fs_err::create_dir_all(&env_dart_tool)
                .context("Failed to create Dart cache directory in env_path")?;
            fs_err::copy(
                &source_package_config,
                env_dart_tool.join("package_config.json"),
            )
            .context("Failed to copy package_config.json to env_path")?;
        }

        let pubspec_path = repo_path.join("pubspec.yaml");
        let bin_src_dir = repo_path.join("bin");
        let bin_out_dir = env_path.join("bin");
        Self::compile_executables(dart, &pubspec_path, &bin_src_dir, &bin_out_dir, env_path)
            .await?;

        Ok(())
    }

    async fn install_additional_dependencies(
        dart: &Path,
        env_path: &Path,
        dependencies: &rustc_hash::FxHashSet<String>,
    ) -> Result<()> {
        let mut pubspec_content = indoc::formatdoc! {"
            name: prek_dart_env
            environment:
              sdk: '>=2.12.0 <4.0.0'
            dependencies:
        "};

        for dep in dependencies {
            if let Some((package, version)) = dep.split_once(':') {
                writeln!(pubspec_content, "  {package}: {version}")?;
            } else {
                writeln!(pubspec_content, "  {dep}: any")?;
            }
        }

        let pubspec_path = env_path.join("pubspec.yaml");
        fs_err::tokio::write(&pubspec_path, pubspec_content).await?;

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
