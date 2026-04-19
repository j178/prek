use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::str;
use std::sync::Arc;

use anyhow::{Context, Result};
use prek_consts::env_vars::EnvVars;
use prek_consts::prepend_paths;
use semver::Version;
use serde_json::json;
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

#[derive(Debug)]
struct PubspecInfo {
    package_name: String,
    executables: Vec<ExecutableInfo>,
}

#[derive(Debug)]
struct ExecutableInfo {
    entrypoint: String,
    output_name: String,
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
            Self::install_from_pubspec(
                &dart_info.executable,
                &info.env_path,
                source_path,
                &hook.additional_dependencies,
            )
            .await?;
        } else if !hook.additional_dependencies.is_empty() {
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
        let packages_path = Self::package_config_path(env_dir);
        let entry = Self::with_packages_file(hook.entry.resolve(Some(&new_path))?, &packages_path);

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
    fn package_config_path(env_path: &Path) -> PathBuf {
        env_path.join(".dart_tool").join("package_config.json")
    }

    fn with_packages_file(mut entry: Vec<String>, packages_path: &Path) -> Vec<String> {
        if !packages_path.exists() || entry.is_empty() {
            return entry;
        }

        let Some(dart_index) = entry.iter().position(|arg| {
            Path::new(arg)
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name == "dart" || name == "dart.exe")
        }) else {
            return entry;
        };
        if entry
            .iter()
            .any(|arg| arg == "-p" || arg.starts_with("--packages="))
        {
            return entry;
        }

        let Some(target_index) = entry
            .iter()
            .enumerate()
            .skip(dart_index + 1)
            .find_map(|(index, arg)| (!arg.starts_with('-')).then_some((index, arg)))
        else {
            return entry;
        };

        let (_, target) = target_index;
        if *target != "run"
            && !Path::new(target)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("dart"))
        {
            return entry;
        }

        let (index, _) = target_index;
        entry.insert(index, format!("--packages={}", packages_path.display()));
        entry
    }

    fn read_pubspec_info(pubspec_path: &Path) -> Result<PubspecInfo> {
        let pubspec_content =
            fs_err::read_to_string(pubspec_path).context("Failed to read pubspec.yaml")?;
        let pubspec: serde_json::Value =
            serde_saphyr::from_str(&pubspec_content).context("Failed to parse pubspec.yaml")?;

        let package_name = pubspec
            .get("name")
            .and_then(serde_json::Value::as_str)
            .context("pubspec.yaml must define a package name")?
            .to_string();

        let executables = match pubspec.get("executables") {
            None => Vec::new(),
            Some(executables) => {
                let executables = executables
                    .as_object()
                    .context("pubspec.yaml executables must be a mapping")?;

                executables
                    .iter()
                    .map(|(output_name, value)| {
                        let entrypoint = match value {
                            serde_json::Value::Null => output_name.clone(),
                            serde_json::Value::String(entrypoint) if !entrypoint.is_empty() => {
                                entrypoint.clone()
                            }
                            serde_json::Value::String(_) => output_name.clone(),
                            _ => anyhow::bail!(
                                "pubspec.yaml executable `{output_name}` must map to a string or null"
                            ),
                        };

                        Ok(ExecutableInfo {
                            entrypoint,
                            output_name: output_name.clone(),
                        })
                    })
                    .collect::<Result<Vec<_>>>()?
            }
        };

        Ok(PubspecInfo {
            package_name,
            executables,
        })
    }

    async fn compile_executables(
        dart: &Path,
        source_path: &Path,
        bin_out_dir: &Path,
        packages_path: &Path,
        executables: &[ExecutableInfo],
    ) -> Result<()> {
        if executables.is_empty() {
            return Ok(());
        }

        fs_err::create_dir_all(bin_out_dir).context("Failed to create bin output directory")?;

        for executable in executables {
            let mut relative_entrypoint = PathBuf::from(&executable.entrypoint);
            if relative_entrypoint.extension().is_none() {
                relative_entrypoint.set_extension("dart");
            }
            let source_file = source_path.join("bin").join(relative_entrypoint);
            if !source_file.exists() {
                debug!(
                    "Skipping executable '{}': source file not found",
                    executable.output_name
                );
                continue;
            }

            let output_path = if cfg!(windows) {
                bin_out_dir.join(format!("{}.exe", executable.output_name))
            } else {
                bin_out_dir.join(&executable.output_name)
            };

            debug!(
                "Compiling executable '{exe_name}': {source} -> {output}",
                exe_name = executable.output_name,
                source = source_file.display(),
                output = output_path.display(),
            );

            Cmd::new(dart, "dart compile exe")
                .arg("compile")
                .arg("exe")
                .arg(format!("--packages={}", packages_path.display()))
                .arg(&source_file)
                .arg("--output")
                .arg(&output_path)
                .check(true)
                .output()
                .await
                .with_context(|| {
                    format!("Failed to compile executable '{}'", executable.output_name)
                })?;
        }

        Ok(())
    }

    fn build_env_pubspec(
        source_path: Option<&Path>,
        package_name: Option<&str>,
        dependencies: &rustc_hash::FxHashSet<String>,
    ) -> Result<String> {
        let mut resolved_dependencies = serde_json::Map::new();

        for dep in dependencies {
            if let Some((package, version)) = dep.split_once(':') {
                resolved_dependencies.insert(package.to_string(), json!(version));
            } else {
                resolved_dependencies.insert(dep.clone(), json!("any"));
            }
        }

        if let (Some(source_path), Some(package_name)) = (source_path, package_name) {
            resolved_dependencies.insert(
                package_name.to_string(),
                json!({ "path": source_path.to_string_lossy().to_string() }),
            );
        }

        let pubspec = json!({
            "name": "prek_dart_env",
            "environment": {
                "sdk": ">=2.12.0 <4.0.0"
            },
            "dependencies": resolved_dependencies,
        });

        serde_saphyr::to_string(&pubspec).context("Failed to build pubspec.yaml")
    }

    async fn install_package_config(
        dart: &Path,
        env_path: &Path,
        source_path: Option<&Path>,
        package_name: Option<&str>,
        dependencies: &rustc_hash::FxHashSet<String>,
    ) -> Result<()> {
        let pubspec_content = Self::build_env_pubspec(source_path, package_name, dependencies)?;
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
            .context("Failed to run dart pub get")?;

        Ok(())
    }

    async fn install_from_pubspec(
        dart: &Path,
        env_path: &Path,
        source_path: &Path,
        dependencies: &rustc_hash::FxHashSet<String>,
    ) -> Result<()> {
        let pubspec_info = Self::read_pubspec_info(&source_path.join("pubspec.yaml"))?;
        Self::install_package_config(
            dart,
            env_path,
            Some(source_path),
            Some(&pubspec_info.package_name),
            dependencies,
        )
        .await?;

        let bin_out_dir = env_path.join("bin");
        Self::compile_executables(
            dart,
            source_path,
            &bin_out_dir,
            &Self::package_config_path(env_path),
            &pubspec_info.executables,
        )
        .await?;

        Ok(())
    }

    async fn install_additional_dependencies(
        dart: &Path,
        env_path: &Path,
        dependencies: &rustc_hash::FxHashSet<String>,
    ) -> Result<()> {
        Self::install_package_config(dart, env_path, None, None, dependencies)
            .await
            .context("Failed to run dart pub get for additional dependencies")
    }

    fn has_pubspec(repo_path: &Path) -> bool {
        repo_path.join("pubspec.yaml").exists()
    }
}
