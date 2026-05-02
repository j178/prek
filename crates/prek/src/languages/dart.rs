//! A Dart package is described by `pubspec.yaml`. The package `name` is what
//! other packages depend on, `dependencies` are resolved by `dart pub get`, and
//! `executables` declares command names that map to Dart files under `bin/`.
//! For executable entries, Dart treats a null value as "use the command name as
//! the entrypoint"; this module also treats an empty string that way.
//!
//! `dart pub get` writes `.dart_tool/package_config.json`, which is the package
//! resolver map used by the Dart VM. `prek` creates a pubspec in the hook env,
//! runs `dart pub get` there with `PUB_CACHE` pointed at the env, and passes the
//! generated package config to hook commands that run `dart run` or direct
//! `.dart` scripts.

use std::collections::BTreeMap;
use std::env::consts::EXE_EXTENSION;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use anyhow::{Context, Result};
use prek_consts::env_vars::EnvVars;
use prek_consts::prepend_paths;
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::cli::reporter::{HookInstallReporter, HookRunReporter};
use crate::hook::{Hook, InstallInfo, InstalledHook};
use crate::languages::LanguageImpl;
use crate::process::Cmd;
use crate::run::run_by_batch;
use crate::store::Store;

#[derive(Debug, Copy, Clone)]
pub(crate) struct Dart;

const PUBSPEC_YAML: &str = "pubspec.yaml";

/// Dart package manifest data from `pubspec.yaml`.
///
/// Format reference: <https://dart.dev/tools/pub/pubspec>.
#[derive(Debug, Deserialize, Serialize)]
struct Pubspec {
    name: String,
    #[serde(
        default,
        skip_deserializing,
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    environment: BTreeMap<String, String>,
    #[serde(
        default,
        skip_deserializing,
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    dependencies: BTreeMap<String, PubspecDependency>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    executables: BTreeMap<String, PubspecExecutable>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(untagged)]
enum PubspecExecutable {
    Entrypoint(String),
    Default,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(untagged)]
enum PubspecDependency {
    Version(String),
    Path { path: PathBuf },
}

impl PubspecExecutable {
    /// Convert Dart's executable shorthand into the entrypoint name under `bin/`.
    fn into_entrypoint(self, output_name: &str) -> String {
        match self {
            Self::Entrypoint(entrypoint) if !entrypoint.is_empty() => entrypoint,
            Self::Entrypoint(_) | Self::Default => output_name.to_string(),
        }
    }
}

/// Resolve the Dart binary that should own this hook environment.
fn find_dart_binary() -> Result<PathBuf> {
    let dart = which::which("dart")
        .context("Failed to locate dart executable. Is Dart installed and available in PATH?")?;
    Ok(dart)
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

        let dart = find_dart_binary()?;

        let source_path = hook.repo_path().unwrap_or_else(|| hook.work_dir());
        if source_path.join(PUBSPEC_YAML).exists() {
            install_from_pubspec(
                &dart,
                &info.env_path,
                source_path,
                &hook.additional_dependencies,
            )
            .await?;
        } else if !hook.additional_dependencies.is_empty() {
            install_package_config(
                &dart,
                &info.env_path,
                None,
                None,
                &hook.additional_dependencies,
            )
            .await
            .context("Failed to install Dart additional dependencies")?;
        }

        info.with_toolchain(dart);
        info.persist_env_path();

        reporter.on_install_complete(progress);

        Ok(InstalledHook::Installed {
            hook,
            info: Arc::new(info),
        })
    }

    async fn check_health(&self, info: &InstallInfo) -> Result<()> {
        let dart = find_dart_binary()?;

        if dart != info.toolchain {
            anyhow::bail!(
                "Dart executable mismatch: expected `{}`, found `{}`",
                info.toolchain.display(),
                dart.display()
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

        let env_dir = hook.env_path().expect("Dart must have env path");
        let bin_path = bin_path(env_dir);
        let new_path = prepend_paths(&[&bin_path]).context("Failed to join PATH")?;
        let packages_path = package_config_path(env_dir);

        let mut entry = hook.entry.resolve(Some(&new_path), store)?;
        // `dart pub get` writes the hook env's dependency graph here. Dart's
        // VM-level `--packages` flag makes `Platform.packageConfig` and package
        // imports resolve against this env instead of the hook work dir.
        if packages_path.exists()
            && let Some(index) = packages_arg_insert_position(entry.argv(), &hook.args)
        {
            entry
                .argv_mut()
                .insert(index, format!("--packages={}", packages_path.display()));
        }

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

        let results = run_by_batch(hook, filenames, entry.argv(), run).await?;

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

/// Return the env directory containing compiled Dart executables.
fn bin_path(env_path: &Path) -> PathBuf {
    env_path.join("bin")
}

/// Return the package config generated by `dart pub get` inside an env.
fn package_config_path(env_path: &Path) -> PathBuf {
    env_path.join(".dart_tool").join("package_config.json")
}

/// Return the `entry` argv position where `--packages=...` should be inserted.
fn packages_arg_insert_position(entry: &[String], hook_args: &[String]) -> Option<usize> {
    fn is_dart_binary(arg: &str) -> bool {
        Path::new(arg)
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| matches!(name, "dart" | "dart.exe"))
    }

    fn has_packages_arg(arg: &str) -> bool {
        arg == "-p" || arg == "--packages" || arg.starts_with("--packages=")
    }

    fn is_dart_script(arg: &str) -> bool {
        Path::new(arg)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("dart"))
    }

    let dart_index = entry.iter().position(|arg| is_dart_binary(arg))?;

    for (index, arg) in entry
        .iter()
        .chain(hook_args)
        .enumerate()
        .skip(dart_index + 1)
    {
        // Respect an explicit package config only while still parsing Dart VM
        // options. After `run` or a script target, this may be a hook argument.
        if has_packages_arg(arg) {
            return None;
        }

        if !arg.starts_with('-') {
            // `--packages` is a VM flag, so place it before `run` or a script target.
            if arg != "run" && !is_dart_script(arg) {
                return None;
            }
            return Some(index.min(entry.len()));
        }
    }

    None
}

/// Compile declared package executables into the hook env's `bin` directory.
async fn compile_executables(
    dart: &Path,
    source_path: &Path,
    bin_dir: &Path,
    packages_path: &Path,
    executables: BTreeMap<String, PubspecExecutable>,
) -> Result<()> {
    if executables.is_empty() {
        return Ok(());
    }

    fs_err::create_dir_all(bin_dir)?;

    for (output_name, executable) in executables {
        let entrypoint = executable.into_entrypoint(&output_name);
        let mut relative_entrypoint = PathBuf::from(&entrypoint);
        if relative_entrypoint.extension().is_none() {
            relative_entrypoint.set_extension("dart");
        }
        let source_file = source_path.join("bin").join(relative_entrypoint);
        if !source_file.exists() {
            debug!("Skipping executable `{output_name}`: source file not found");
            continue;
        }

        let output_path = bin_dir.join(&output_name).with_extension(EXE_EXTENSION);

        debug!(
            "Compiling executable `{output_name}`: {source} -> {output}",
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
            .await?;
    }

    Ok(())
}

/// Build the synthetic pubspec used to resolve the hook's Dart dependencies.
fn build_env_pubspec(
    source_path: Option<&Path>,
    package_name: Option<&str>,
    dependencies: &rustc_hash::FxHashSet<String>,
) -> Pubspec {
    let mut resolved_dependencies = BTreeMap::new();

    let mut dependencies = dependencies.iter().collect::<Vec<_>>();
    dependencies.sort_unstable();

    for dep in dependencies {
        if let Some((package, version)) = dep.split_once(':') {
            resolved_dependencies.insert(
                package.to_string(),
                PubspecDependency::Version(version.to_string()),
            );
        } else {
            resolved_dependencies
                .insert(dep.clone(), PubspecDependency::Version("any".to_string()));
        }
    }

    if let (Some(source_path), Some(package_name)) = (source_path, package_name) {
        resolved_dependencies.insert(
            package_name.to_string(),
            PubspecDependency::Path {
                path: source_path.to_path_buf(),
            },
        );
    }

    Pubspec {
        name: "prek_dart_env".to_string(),
        environment: BTreeMap::from([("sdk".to_string(), ">=2.12.0 <4.0.0".to_string())]),
        dependencies: resolved_dependencies,
        executables: BTreeMap::new(),
    }
}

/// Write the synthetic pubspec and resolve it into a package config.
async fn install_package_config(
    dart: &Path,
    env_path: &Path,
    source_path: Option<&Path>,
    package_name: Option<&str>,
    dependencies: &rustc_hash::FxHashSet<String>,
) -> Result<()> {
    let pubspec = build_env_pubspec(source_path, package_name, dependencies);
    let pubspec_content = serde_saphyr::to_string(&pubspec)?;
    let pubspec_path = env_path.join(PUBSPEC_YAML);
    fs_err::tokio::write(&pubspec_path, pubspec_content).await?;

    Cmd::new(dart, "dart pub get")
        .current_dir(env_path)
        .env(EnvVars::PUB_CACHE, env_path)
        .arg("pub")
        .arg("get")
        .check(true)
        .output()
        .await?;

    Ok(())
}

/// Install a local Dart package hook and compile its declared executables.
async fn install_from_pubspec(
    dart: &Path,
    env_path: &Path,
    source_path: &Path,
    dependencies: &rustc_hash::FxHashSet<String>,
) -> Result<()> {
    let pubspec_path = source_path.join(PUBSPEC_YAML);
    let pubspec_content = fs_err::read_to_string(&pubspec_path)?;
    let pubspec: Pubspec = serde_saphyr::from_str(&pubspec_content)?;

    install_package_config(
        dart,
        env_path,
        Some(source_path),
        Some(&pubspec.name),
        dependencies,
    )
    .await
    .context("Failed to install Dart pubspec dependencies")?;

    compile_executables(
        dart,
        source_path,
        &bin_path(env_path),
        &package_config_path(env_path),
        pubspec.executables,
    )
    .await
    .context("Failed to compile Dart pubspec executables")?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn packages_arg_insert_position_inserts_before_dart_run() {
        let entry = strings(&["/usr/bin/dart", "run", "bin/hook.dart"]);

        assert_eq!(packages_arg_insert_position(&entry, &[]), Some(1));
    }

    #[test]
    fn packages_arg_insert_position_keeps_existing_packages_arg() {
        assert_eq!(
            packages_arg_insert_position(&strings(&["dart", "--packages=custom", "run"]), &[]),
            None
        );
        assert_eq!(
            packages_arg_insert_position(&strings(&["dart", "--packages", "custom", "run"]), &[]),
            None
        );
        assert_eq!(
            packages_arg_insert_position(
                &strings(&["dart"]),
                &strings(&["--packages=custom", "run"])
            ),
            None
        );
    }

    #[test]
    fn packages_arg_insert_position_only_checks_vm_options_for_packages_arg() {
        assert_eq!(
            packages_arg_insert_position(&strings(&["dart", "run", "tool.dart", "-p"]), &[]),
            Some(1)
        );
        assert_eq!(
            packages_arg_insert_position(
                &strings(&["dart", "tool.dart", "--packages=script-value"]),
                &[]
            ),
            Some(1)
        );
        assert_eq!(
            packages_arg_insert_position(
                &strings(&["dart"]),
                &strings(&["run", "tool.dart", "--packages=script-value"])
            ),
            Some(1)
        );
    }

    #[test]
    fn packages_arg_insert_position_checks_hook_args() {
        assert_eq!(
            packages_arg_insert_position(&strings(&["dart"]), &strings(&["run", "bin/hook.dart"])),
            Some(1)
        );
        assert_eq!(
            packages_arg_insert_position(&strings(&["dart"]), &strings(&["bin/hook.dart"])),
            Some(1)
        );
        assert_eq!(
            packages_arg_insert_position(
                &strings(&["dart", "--enable-asserts"]),
                &strings(&["run", "bin/hook.dart"])
            ),
            Some(2)
        );
    }

    #[test]
    fn build_env_pubspec_serializes_path_dependency() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let dependencies = rustc_hash::FxHashSet::default();

        let pubspec = build_env_pubspec(Some(temp_dir.path()), Some("sample"), &dependencies);

        match pubspec.dependencies.get("sample") {
            Some(PubspecDependency::Path { path }) => assert_eq!(path, temp_dir.path()),
            dependency => panic!("expected path dependency, got {dependency:?}"),
        }

        Ok(())
    }

    #[test]
    fn pubspec_deserialization_ignores_unread_fields() -> Result<()> {
        let pubspec: Pubspec = serde_saphyr::from_str(indoc::indoc! {r"
            name: sample
            environment:
              sdk: '>=2.17.0 <4.0.0'
            dependencies:
              hosted_dep:
                hosted: https://pub.dev
                version: ^1.0.0
              sdk_dep:
                sdk: flutter
            executables:
              sample:
        "})?;

        assert_eq!(pubspec.name, "sample");
        assert!(pubspec.environment.is_empty());
        assert!(pubspec.dependencies.is_empty());
        assert!(pubspec.executables.contains_key("sample"));

        Ok(())
    }
}
