use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use anyhow::{Context, Result};
use constants::env_vars::EnvVars;
use rustc_hash::FxHashSet;
use tokio::io::AsyncWriteExt;
use tracing::debug;

use crate::cli::reporter::HookInstallReporter;
use crate::hook::{Hook, InstallInfo, InstalledHook};
use crate::languages::LanguageImpl;
use crate::languages::python::{Uv, python_exec};
use crate::process::Cmd;
use crate::run::CONCURRENCY;
use crate::store::{CacheBucket, Store, ToolBucket};

#[derive(Debug, Default)]
struct Args {
    ignore_case: bool,
    multiline: bool,
    negate: bool,
}

impl Args {
    fn parse(args: &[String]) -> Result<Self> {
        let mut parsed = Args::default();

        for arg in args {
            match arg.as_str() {
                "--ignore-case" | "-i" => parsed.ignore_case = true,
                "--multiline" => parsed.multiline = true,
                "--negate" => parsed.negate = true,
                _ => anyhow::bail!("Unknown argument: {}", arg),
            }
        }

        Ok(parsed)
    }

    fn to_args(&self) -> Vec<&'static str> {
        fn as_str(value: bool) -> &'static str {
            if value { "1" } else { "0" }
        }
        vec![
            as_str(self.ignore_case),
            as_str(self.multiline),
            as_str(self.negate),
        ]
    }
}

#[derive(serde::Deserialize, thiserror::Error, Debug)]
#[serde(tag = "type")]
enum Error {
    #[error("Failed to parse regex: {message}")]
    Regex { message: String },
    #[error("IO error: {message}")]
    IO { message: String },
    #[error("Unknown error: {message}")]
    Unknown { message: String },
}

// We have to implement `pygrep` in Python, because Python `re` module has many differences
// from Rust `regex` crate.
static SCRIPT: &str = include_str!("script.py");

const INSTALL_PYTHON_VERSION: &str = "3.13";

pub(crate) struct Pygrep;

fn find_installed_python(python_dir: &Path) -> Option<PathBuf> {
    fs_err::read_dir(python_dir)
        .ok()
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|f| f.is_dir()))
        .map(|entry| python_exec(&entry.path()))
        .next()
}

impl LanguageImpl for Pygrep {
    async fn install(
        &self,
        hook: Arc<Hook>,
        store: &Store,
        reporter: &HookInstallReporter,
    ) -> Result<InstalledHook> {
        let progress = reporter.on_install_start(&hook);

        let uv_dir = store.tools_path(ToolBucket::Uv);
        let uv = Uv::install(&uv_dir).await?;
        let python_dir = store.tools_path(ToolBucket::Python);

        // Find or download a Python interpreter.
        let mut python = None;

        // 1) Try to find one from `prek` managed Python versions.
        if let Some(installed) = find_installed_python(&python_dir) {
            python = Some(installed);
        } else {
            // 2) If not found, try to find a system installed Python (system or system uv managed).
            debug!("No Python interpreter found, trying to find a system installed one");
            let mut output = uv
                .cmd("uv python find", store)
                .arg("python")
                .arg("find")
                .arg("--python-preference")
                .arg("managed")
                .arg("--no-python-downloads")
                .arg("--no-config")
                .arg("--no-project")
                .check(false)
                .output()
                .await?;
            if output.status.success() {
                python = Some(PathBuf::from(
                    String::from_utf8_lossy(&output.stdout).trim(),
                ));
            } else {
                // 3) If still not found, try to download a Python interpreter.
                debug!("No Python interpreter found, trying to install one");
                output = uv
                    .cmd("uv python install", store)
                    .arg("python")
                    .arg("install")
                    .arg(INSTALL_PYTHON_VERSION)
                    .arg("--no-config")
                    .arg("--no-project")
                    .env(EnvVars::UV_PYTHON_INSTALL_DIR, &python_dir)
                    .check(false)
                    .output()
                    .await?;
                if output.status.success() {
                    if let Some(installed) = find_installed_python(&python_dir) {
                        python = Some(installed);
                    }
                }
            }
        }

        let Some(python) = python else {
            anyhow::bail!("Failed to find or install a Python interpreter for `pygrep`.");
        };

        let mut info = InstallInfo::new(hook.language, FxHashSet::default(), &store.hooks_dir());
        info.with_toolchain(python);

        fs_err::tokio::create_dir_all(&info.env_path).await?;

        reporter.on_install_complete(progress);

        Ok(InstalledHook::Installed {
            hook,
            info: Arc::new(info),
        })
    }

    async fn check_health(&self) -> Result<()> {
        todo!()
    }

    async fn run(
        &self,
        hook: &InstalledHook,
        filenames: &[&String],
        store: &Store,
    ) -> Result<(i32, Vec<u8>)> {
        let InstalledHook::Installed { info, .. } = hook else {
            unreachable!()
        };

        let cache = store.cache_path(CacheBucket::Python);
        fs_err::tokio::create_dir_all(&cache).await?;

        let py_script = tempfile::NamedTempFile::new_in(cache)?;
        fs_err::tokio::write(&py_script, SCRIPT)
            .await
            .context("Failed to write Python script")?;

        let args = Args::parse(&hook.args).context("Failed to parse `args`")?;
        let mut cmd = Cmd::new(&info.toolchain, "python script")
            .arg("-I") // Isolate mode.
            .arg("-B") // Don't write bytecode.
            .arg(py_script.path())
            .args(args.to_args())
            .arg(CONCURRENCY.to_string())
            .arg(hook.entry.raw())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .check(false)
            .spawn()?;

        let mut stdin = cmd.stdin.take().context("Failed to take stdin")?;
        // TODO: avoid this clone if possible.
        let filenames: Vec<_> = filenames.iter().map(ToString::to_string).collect();

        let write_task = tokio::spawn(async move {
            for filename in filenames {
                stdin.write_all(format!("{filename}\n").as_bytes()).await?;
            }
            let _ = stdin.shutdown().await;
            anyhow::Ok(())
        });

        let output = cmd
            .wait_with_output()
            .await
            .context("Failed to wait for command output")?;
        write_task.await.context("Failed to write stdin")??;

        if output.status.success() {
            // When successful, the Python script writes status code JSON to stderr
            // and grep results to stdout
            let stderr_str = String::from_utf8_lossy(&output.stderr);
            let code_output: serde_json::Value =
                serde_json::from_str(&stderr_str).with_context(|| {
                    format!(
                        "Failed to parse status code JSON from stderr. Stderr content: '{stderr_str}'",
                    )
                })?;
            let code = code_output
                .get("code")
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(0);
            let code = i32::try_from(code).unwrap_or(0);
            Ok((code, output.stdout))
        } else {
            // When there's an error, try to parse error JSON from stderr
            let stderr_str = String::from_utf8_lossy(&output.stderr);

            if stderr_str.trim().is_empty() {
                // No stderr output - create a generic error
                return Err(anyhow::anyhow!(
                    "Python script failed with exit code {} but produced no error output",
                    output.status.code().unwrap_or(-1)
                ));
            }

            // Try to parse as JSON first
            match serde_json::from_str::<Error>(&stderr_str) {
                Ok(err) => Err(err.into()),
                Err(_) => {
                    // Not JSON - treat as plain text error message
                    Err(anyhow::anyhow!(
                        "Python script failed with exit code {}: {}",
                        output.status.code().unwrap_or(-1),
                        stderr_str.trim()
                    ))
                }
            }
        }
    }
}
