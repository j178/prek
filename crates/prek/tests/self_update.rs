#![cfg(feature = "self-update")]

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use assert_cmd::assert::OutputAssertExt;
use serde_json::json;

use crate::common::{TestEnv, cmd_snapshot};

mod common;

struct Server {
    address: SocketAddr,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<Result<()>>>,
}

impl Server {
    fn new(routes: impl FnOnce(&str) -> HashMap<String, String>) -> Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let address = listener.local_addr()?;
        let routes = routes(&format!("http://{address}"));
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = Arc::clone(&stop);
        let worker = std::thread::spawn(move || {
            loop {
                let (mut connection, _) = listener.accept()?;
                if stopping.load(Ordering::Relaxed) {
                    return Ok(());
                }
                connection.set_read_timeout(Some(Duration::from_secs(10)))?;
                let mut reader = BufReader::new(&connection);
                let mut request = String::new();
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line)? == 0 || line == "\r\n" {
                        break;
                    }
                    request.push_str(&line);
                }
                let path = request
                    .split_ascii_whitespace()
                    .nth(1)
                    .context("Missing request path")?;
                let body = routes
                    .get(path)
                    .with_context(|| format!("Unexpected request: {path}"))?;
                write!(
                    connection,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len(),
                )?;
            }
        });
        Ok(Self {
            address,
            stop,
            worker: Some(worker),
        })
    }

    fn url(&self) -> String {
        format!("http://{}", self.address)
    }

    fn finish(mut self) -> Result<()> {
        self.stop.store(true, Ordering::Relaxed);
        let _ = TcpStream::connect(self.address);
        match self.worker.take().context("Missing server worker")?.join() {
            Ok(result) => result,
            Err(_) => bail!("HTTP server panicked"),
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = TcpStream::connect(self.address);
    }
}

fn release(base: &str, tag: &str) -> String {
    json!({
        "tag_name": tag,
        "prerelease": false,
        "assets": [{
            "name": if cfg!(windows) { "prek-installer.ps1" } else { "prek-installer.sh" },
            "browser_download_url": format!("{base}/installer"),
        }],
    })
    .to_string()
}

fn installation(context: &TestEnv, version: &str) -> Result<PathBuf> {
    let prefix = context.work_dir().join("install");
    fs_err::create_dir_all(prefix.join("bin"))?;
    let executable = prefix.join(format!("bin/prek{}", std::env::consts::EXE_SUFFIX));
    fs_err::copy(assert_cmd::cargo::cargo_bin!("prek"), &executable)?;
    context.write_file(
        "prek-receipt.json",
        serde_json::to_vec(&json!({
            "install_prefix": prefix,
            "version": version,
            "provider": {"source": "cargo-dist", "version": "0.30.0"},
            "source": {
                "release_type": "github", "owner": "j178", "name": "prek", "app_name": "prek"
            },
        }))?,
    );
    Ok(executable)
}

fn command(context: &TestEnv, executable: &Path, server: &Server) -> Command {
    let mut command = Command::new(executable);
    command
        .current_dir(context.work_dir())
        .args(["self", "update"])
        .env("PREK_HOME", context.home_dir().as_ref())
        .env("AXOUPDATER_CONFIG_PATH", context.work_dir().as_ref())
        .env_remove("AXOUPDATER_CONFIG_WORKING_DIR")
        .env("PREK_INSTALLER_GHE_BASE_URL", server.url())
        .env_remove("PREK_INSTALLER_GITHUB_BASE_URL")
        .env_remove("PREK_GITHUB_TOKEN")
        .env_remove("GITHUB_TOKEN")
        .env_remove("RUST_LOG")
        .env("NO_PROXY", "127.0.0.1");
    command
}

/// Updating downloads and runs the installer, reports success, and leaves a working executable.
#[test]
fn updates_to_latest() -> Result<()> {
    let context = TestEnv::new();
    let executable = installation(&context, "0.1.0")?;
    let server = Server::new(|base| {
        let installer = if cfg!(windows) {
            indoc::indoc! {r#"
                $ErrorActionPreference = 'Stop'
                Copy-Item "$env:PREK_INSTALL_DIR/bin/prek.exe.previous.exe" "$env:PREK_INSTALL_DIR/bin/prek.exe"
                [System.IO.File]::WriteAllText("$env:PREK_INSTALL_DIR/updated", "updated")
            "#}
        } else {
            indoc::indoc! {r#"
                #!/bin/sh
                set -eu
                printf updated > "$PREK_INSTALL_DIR/updated"
            "#}
        };
        HashMap::from([
            (
                "/api/v3/repos/j178/prek/releases/latest".into(),
                release(base, "v0.2.0"),
            ),
            ("/installer".into(), installer.into()),
        ])
    })?;
    cmd_snapshot!(context, command(&context, &executable, &server), @"
    success: true
    exit_code: 0
    ----- stdout -----

    ----- stderr -----
    info: Checking for updates...
    success: Upgraded prek from v0.1.0 to v0.2.0! https://github.com/j178/prek/releases/tag/v0.2.0
    ");
    assert_eq!(context.read("install/updated"), "updated");
    Command::new(&executable)
        .arg("--version")
        .assert()
        .success();
    server.finish()
}

/// When already current, report that no update is needed without downloading an installer.
#[test]
fn already_up_to_date() -> Result<()> {
    let context = TestEnv::new().with_filter(env!("CARGO_PKG_VERSION"), "[VERSION]");
    let executable = installation(&context, "0.2.0")?;
    let server = Server::new(|base| {
        HashMap::from([(
            "/api/v3/repos/j178/prek/releases/latest".into(),
            release(base, "v0.2.0"),
        )])
    })?;
    cmd_snapshot!(context, command(&context, &executable, &server), @"
    success: true
    exit_code: 0
    ----- stdout -----

    ----- stderr -----
    info: Checking for updates...
    success: You're on the latest version of prek (v[VERSION])
    ");
    server.finish()
}
