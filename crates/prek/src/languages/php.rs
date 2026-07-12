use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use anyhow::{Context, Result};
use prek_consts::env_vars::EnvVars;
use prek_consts::prepend_paths;
use semver::Version;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use tracing::debug;

use crate::cli::reporter::HookInstallReporter;
use crate::cli::run::HookRunReporter;
use crate::hook::{Hook, InstallInfo, InstalledHook};
use crate::languages::LanguageImpl;
use crate::process::Cmd;
use crate::run::run_by_batch;
use crate::store::Store;

const COMPOSER_JSON: &str = "composer.json";
const COMPOSER_ENVS_TO_REMOVE: &[&str] = &[
    "COMPOSER",
    "COMPOSER_BIN_DIR",
    "COMPOSER_IGNORE_PLATFORM_REQ",
    "COMPOSER_IGNORE_PLATFORM_REQS",
    "COMPOSER_VENDOR_DIR",
];
const HOOK_PACKAGE_VERSION: &str = "dev-prek";

#[derive(Debug, Copy, Clone)]
pub(crate) struct Php;

#[derive(Debug, Deserialize)]
struct ComposerPackage {
    name: String,
}

async fn query_php_version(executable: &Path) -> Result<Version> {
    let output = Cmd::new(executable)
        .arg("-r")
        .arg("echo PHP_VERSION;")
        .check(true)
        .output()
        .await
        .context("Failed to query PHP version")?;
    parse_php_version(&String::from_utf8_lossy(&output.stdout))
}

fn parse_php_version(output: &str) -> Result<Version> {
    let version = output.trim();
    if let Ok(version) = Version::parse(version) {
        return Ok(version);
    }

    let suffix = version
        .find(|character: char| !character.is_ascii_digit() && character != '.')
        .with_context(|| format!("Failed to parse PHP version `{version}`"))?;
    let normalized = format!("{}-{}", &version[..suffix], &version[suffix..]);
    Version::parse(&normalized).with_context(|| format!("Failed to parse PHP version `{version}`"))
}

async fn composer_package_name(repo_path: &Path) -> Result<String> {
    let manifest_path = repo_path.join(COMPOSER_JSON);
    let manifest = fs_err::tokio::read_to_string(&manifest_path)
        .await
        .with_context(|| {
            format!(
                "PHP hook repository must contain `{}`",
                manifest_path.display()
            )
        })?;
    let package: ComposerPackage = serde_json::from_str(&manifest)
        .with_context(|| format!("Failed to parse `{}`", manifest_path.display()))?;

    if package.name.trim().is_empty() {
        anyhow::bail!(
            "Composer package name in `{}` is empty",
            manifest_path.display()
        );
    }

    Ok(package.name)
}

fn composer_manifest(repo: Option<(&Path, &str)>) -> Result<Value> {
    let mut manifest = json!({
        "config": {
            "bin-dir": "bin",
            "vendor-dir": "vendor",
        },
    });

    if let Some((repo_path, package_name)) = repo {
        let repo_path = repo_path.to_str().with_context(|| {
            format!(
                "PHP hook repository path is not UTF-8: {}",
                repo_path.display()
            )
        })?;
        let mut versions = Map::new();
        versions.insert(
            package_name.to_string(),
            Value::String(HOOK_PACKAGE_VERSION.to_string()),
        );
        manifest["repositories"] = json!([{
            "type": "path",
            "url": repo_path,
            "options": {
                "symlink": false,
                "versions": versions,
            },
        }]);
    }

    Ok(manifest)
}

fn bin_dir(env_path: &Path) -> PathBuf {
    env_path.join("bin")
}

fn composer_command(composer: &Path, env_path: &Path, path_env: &OsStr) -> Cmd {
    let mut command = Cmd::new(composer);
    command
        .arg("--no-interaction")
        .arg("--no-ansi")
        .arg("--working-dir")
        .arg(env_path)
        .env(EnvVars::PATH, path_env);
    for &key in COMPOSER_ENVS_TO_REMOVE {
        command.env_remove(key);
    }
    command
}

impl LanguageImpl for Php {
    async fn install(
        &self,
        hook: Arc<Hook>,
        store: &Store,
        reporter: &HookInstallReporter,
    ) -> Result<InstalledHook> {
        let progress = reporter.on_install_start(&hook);
        let mut info = InstallInfo::new(&hook, &store.hooks_dir())?;

        debug!(%hook, target = %info.env_path.display(), "Installing PHP environment");

        let php = which::which("php")
            .context("Failed to locate php executable. Is PHP installed and available in PATH?")?;
        let php_version = query_php_version(&php).await?;
        info.with_language_version(php_version).with_toolchain(php);

        // Composer only creates bin proxies for dependencies, so install remote hooks as
        // mirrored path packages instead of running `composer install` in the hook repository.
        let repo_package = if let Some(repo_path) = hook.repo_path() {
            Some((repo_path, composer_package_name(repo_path).await?))
        } else {
            None
        };

        let mut dependencies = Vec::with_capacity(
            hook.additional_dependencies.len() + usize::from(repo_package.is_some()),
        );
        if let Some((_, package_name)) = &repo_package {
            dependencies.push(format!("{package_name}:{HOOK_PACKAGE_VERSION}"));
        }
        dependencies.extend(hook.additional_dependencies.iter().cloned());

        if !dependencies.is_empty() {
            let composer = which::which("composer").context(
                "Failed to locate composer executable. Is Composer installed and available in PATH?",
            )?;
            let manifest = composer_manifest(
                repo_package
                    .as_ref()
                    .map(|(repo_path, package_name)| (*repo_path, package_name.as_str())),
            )?;
            fs_err::tokio::write(
                info.env_path.join(COMPOSER_JSON),
                serde_json::to_vec_pretty(&manifest)?,
            )
            .await?;

            let php_bin = info
                .toolchain
                .parent()
                .context("PHP executable must have a parent directory")?;
            let path_env = prepend_paths(&[&bin_dir(&info.env_path), php_bin])
                .context("Failed to join PATH")?;

            composer_command(&composer, &info.env_path, &path_env)
                .arg("require")
                .arg("--no-progress")
                .arg("--")
                .args(dependencies)
                .check(true)
                .output()
                .await
                .context("Failed to install PHP dependencies with Composer")?;

            composer_command(&composer, &info.env_path, &path_env)
                .arg("check-platform-reqs")
                .check(true)
                .output()
                .await
                .context("Failed to verify PHP platform requirements with Composer")?;
        }

        info.persist_env_path();
        reporter.on_install_complete(progress);

        Ok(InstalledHook::Installed {
            hook,
            info: Arc::new(info),
        })
    }

    async fn check_health(&self, info: &InstallInfo) -> Result<()> {
        let current_version = query_php_version(&info.toolchain)
            .await
            .context("Failed to query current PHP version")?;

        if current_version != info.language_version {
            anyhow::bail!(
                "PHP version mismatch: expected `{}`, found `{}`",
                info.language_version,
                current_version
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
        let env_path = hook.env_path().expect("PHP must have env path");
        let php_bin = hook
            .toolchain_dir()
            .expect("PHP executable must have a parent directory");
        let path_env =
            prepend_paths(&[&bin_dir(env_path), php_bin]).context("Failed to join PATH")?;
        let entry = hook.entry.resolve(Some(&path_env), store)?;

        let run = async |batch: &[&Path]| {
            let mut output = Cmd::new(&entry[0])
                .current_dir(hook.work_dir())
                .args(&entry[1..])
                .env(EnvVars::PATH, &path_env)
                .envs(&hook.env)
                .args(&hook.args)
                .file_args(batch)
                .check(false)
                .stdin(Stdio::null())
                .pty_output_with_sink(reporter.output_sink(progress))
                .await?;

            reporter.on_run_progress(progress, batch.len() as u64);

            output.stdout.extend(output.stderr);
            let code = output.status.code().unwrap_or(1);
            anyhow::Ok((code, output.stdout))
        };

        let results = run_by_batch(hook, filenames, entry.argv(), run).await?;
        let mut combined_status = 0;
        let mut combined_output = Vec::new();

        for (code, output) in results {
            combined_status |= code;
            combined_output.extend(output);
        }

        reporter.on_run_complete(progress);

        Ok((combined_status, combined_output))
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{composer_manifest, parse_php_version};

    #[test]
    fn parse_php_version_accepts_release_version() {
        let version = parse_php_version("8.4.8\n").unwrap();
        assert_eq!(version.to_string(), "8.4.8");
    }

    #[test]
    fn parse_php_version_normalizes_prerelease_version() {
        let version = parse_php_version("8.5.0RC1").unwrap();
        assert_eq!(version.to_string(), "8.5.0-RC1");
    }

    #[test]
    fn composer_manifest_configures_remote_package_as_mirrored_path_repository() {
        let manifest = composer_manifest(Some((Path::new("/tmp/hook"), "example/hook"))).unwrap();

        assert_eq!(
            manifest,
            serde_json::json!({
                "config": {
                    "bin-dir": "bin",
                    "vendor-dir": "vendor",
                },
                "repositories": [{
                    "type": "path",
                    "url": "/tmp/hook",
                    "options": {
                        "symlink": false,
                        "versions": {"example/hook": "dev-prek"},
                    },
                }],
            })
        );
    }
}
