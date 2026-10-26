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
use serde_json::{Value, json};

use crate::common::{TestEnv, cmd_snapshot};

mod common;

struct Response {
    status: u16,
    headers: String,
    body: String,
}

impl Response {
    fn ok(body: impl Into<String>) -> Self {
        Self {
            status: 200,
            headers: String::new(),
            body: body.into(),
        }
    }
}

struct Server {
    address: SocketAddr,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<Result<Vec<String>>>>,
}

impl Server {
    fn new(routes: impl FnOnce(&str) -> HashMap<String, Response>) -> Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let address = listener.local_addr()?;
        let routes = routes(&format!("http://{address}"));
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = Arc::clone(&stop);
        let worker = std::thread::spawn(move || {
            let mut requests = Vec::new();
            loop {
                let (mut connection, _) = listener.accept()?;
                if stopping.load(Ordering::Relaxed) {
                    return Ok(requests);
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
                let response = routes
                    .get(path)
                    .with_context(|| format!("Unexpected request: {path}"))?;
                write!(
                    connection,
                    "HTTP/1.1 {} Test\r\nContent-Length: {}\r\nConnection: close\r\n{}\r\n{}",
                    response.status,
                    response.body.len(),
                    response.headers,
                    response.body,
                )?;
                requests.push(request);
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

    fn finish(mut self) -> Result<Vec<String>> {
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

fn release(base: &str, tag: &str) -> Value {
    json!({
        "tag_name": tag,
        "name": tag,
        "url": format!("{base}/api/v3/repos/j178/prek/releases/1"),
        "prerelease": false,
        "assets": [{
            "name": if cfg!(windows) { "prek-installer.ps1" } else { "prek-installer.sh" },
            "url": format!("{base}/api/v3/repos/j178/prek/releases/assets/1"),
            "browser_download_url": format!("{base}/installer"),
        }],
    })
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
            "binaries": ["prek"],
            "version": version,
            "provider": {"source": "cargo-dist", "version": "0.30.0"},
            "source": {
                "release_type": "github", "owner": "j178", "name": "prek", "app_name": "prek"
            },
        }))?,
    );
    Ok(executable)
}

fn successful_installer() -> &'static str {
    if cfg!(windows) {
        indoc::indoc! {r#"
            $ErrorActionPreference = 'Stop'
            Copy-Item "$env:PREK_INSTALL_DIR/bin/prek.exe.previous.exe" "$env:PREK_INSTALL_DIR/bin/prek.exe"
        "#}
    } else {
        "#!/bin/sh\nexit 0\n"
    }
}

fn check_executable(executable: &Path) -> Result<()> {
    let output = Command::new(executable).arg("--version").output()?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
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

/// A legacy receipt must retain its installation root and PATH preference when updating.
/// The real installer process writes the received directory so the test checks the handoff.
#[test]
fn updates_legacy_installation() -> Result<()> {
    let context = TestEnv::new();
    let executable = installation(&context, "0.1.0")?;
    let mut receipt: Value = serde_json::from_str(&context.read("prek-receipt.json"))?;
    receipt["install_prefix"] = json!(context.work_dir().join("install/bin"));
    receipt["provider"]["version"] = json!("0.14.0");
    receipt["modify_path"] = json!(false);
    context.write_file("prek-receipt.json", serde_json::to_vec(&receipt)?);

    let server = Server::new(|base| {
        HashMap::from([
            (
                "/api/v3/repos/j178/prek/releases/latest".into(),
                Response::ok(release(base, "v0.2.0").to_string()),
            ),
            (
                "/installer".into(),
                Response::ok(if cfg!(windows) {
                    indoc::indoc! {r#"
                        $ErrorActionPreference = 'Stop'
                        if ($env:CARGO_DIST_FORCE_INSTALL_DIR -ne $env:PREK_INSTALL_DIR) { throw 'Wrong installation root' }
                        if ($env:PREK_NO_MODIFY_PATH -ne '1') { throw 'Wrong PATH preference' }
                        Copy-Item "$env:PREK_INSTALL_DIR/bin/prek.exe.previous.exe" "$env:PREK_INSTALL_DIR/bin/prek.exe"
                        [System.IO.File]::WriteAllText("$env:PREK_INSTALL_DIR/updated", $env:PREK_INSTALL_DIR)
                    "#}
                } else {
                    indoc::indoc! {r#"
                        #!/bin/sh
                        set -eu
                        test "$CARGO_DIST_FORCE_INSTALL_DIR" = "$PREK_INSTALL_DIR"
                        test "$PREK_NO_MODIFY_PATH" = 1
                        test -z "${PSModulePath+x}"
                        printf '%s' "$PREK_INSTALL_DIR" > "$PREK_INSTALL_DIR/updated"
                    "#}
                }),
            ),
        ])
    })?;
    let mut command = command(&context, &executable, &server);
    command
        .args(["--token", "test-token"])
        .env("PSModulePath", "inherited");
    cmd_snapshot!(context, command, @"
    success: true
    exit_code: 0
    ----- stdout -----

    ----- stderr -----
    info: Checking for updates...
    success: Upgraded prek from v0.1.0 to v0.2.0! https://github.com/j178/prek/releases/tag/v0.2.0
    ");
    assert_eq!(
        context.read("install/updated"),
        context.work_dir().join("install").to_string_lossy()
    );
    check_executable(&executable)?;
    let requests = server.finish()?;
    let authorization = requests
        .iter()
        .map(|request| {
            request
                .lines()
                .find(|line| line.starts_with("authorization:"))
        })
        .collect::<Vec<_>>();
    assert_eq!(
        authorization,
        [Some("authorization: Bearer test-token"), None]
    );
    Ok(())
}

/// If latest lacks an installer, pagination must find the highest stable installable version.
#[test]
fn latest_falls_back_to_paginated_releases() -> Result<()> {
    let context = TestEnv::new();
    let executable = installation(&context, "0.1.0")?;
    let server = Server::new(|base| {
        let mut missing_installer = release(base, "v3.0.0");
        missing_installer["assets"] = json!([]);
        let mut prerelease = release(base, "v4.0.0-beta.1");
        prerelease["prerelease"] = json!(true);
        let mut first_page = Response::ok(
            json!([
                release(base, "v0.2.0"),
                missing_installer,
                prerelease,
                release(base, "invalid-tag")
            ])
            .to_string(),
        );
        first_page.headers = format!("Link: <{base}/page2>; rel=\"next\"\r\n");
        HashMap::from([
            (
                "/api/v3/repos/j178/prek/releases/latest".into(),
                Response::ok(missing_installer.to_string()),
            ),
            ("/api/v3/repos/j178/prek/releases".into(), first_page),
            (
                "/page2".into(),
                Response::ok(json!([release(base, "prek-v0.3.0")]).to_string()),
            ),
            ("/installer".into(), Response::ok(successful_installer())),
        ])
    })?;
    cmd_snapshot!(context, command(&context, &executable, &server), @"
    success: true
    exit_code: 0
    ----- stdout -----

    ----- stderr -----
    info: Checking for updates...
    success: Upgraded prek from v0.1.0 to v0.3.0! https://github.com/j178/prek/releases/tag/prek-v0.3.0
    ");
    check_executable(&executable)?;
    assert_eq!(server.finish()?.len(), 4);
    Ok(())
}

/// Explicit tags allow downgrades, while a failed installer must surface its captured stderr.
#[test]
fn downgrade_reports_installer_failure() -> Result<()> {
    let context = TestEnv::new().with_filter("exit code: 17", "exit status: 17");
    let executable = installation(&context, "0.3.0")?;
    let server = Server::new(|base| {
        HashMap::from([
            (
                "/api/v3/repos/j178/prek/releases/tags/v0.2.0".into(),
                Response::ok(release(base, "v0.2.0").to_string()),
            ),
            (
                "/installer".into(),
                Response::ok(if cfg!(windows) {
                    indoc::indoc! {r#"
                        $ErrorActionPreference = 'Stop'
                        Set-Content "$env:PREK_INSTALL_DIR/bin/prek.exe" 'incomplete installation'
                        [Console]::Error.WriteLine('simulated install failure')
                        exit 17
                    "#}
                } else {
                    "#!/bin/sh\nprintf 'simulated install failure\\n' >&2\nexit 17\n"
                }),
            ),
        ])
    })?;
    cmd_snapshot!(context, command(&context, &executable, &server).arg("v0.2.0"), @"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    info: Checking for updates...
    error: The installation failed (exit status: 17). Output from the installer:
    simulated install failure
    ");
    assert_eq!(server.finish()?.len(), 2);
    // On Windows, the failing installer overwrote the destination; the restored binary must run.
    check_executable(&executable)?;
    Ok(())
}

/// Failure to start the installer must also restore the running executable on Windows.
#[test]
fn installer_launch_failure_preserves_executable() -> Result<()> {
    let context = TestEnv::new().with_filter(
        r"(?s)(error: Failed to execute the prek installer).*",
        "$1\n",
    );
    let executable = installation(&context, "0.1.0")?;
    let server = Server::new(|base| {
        HashMap::from([
            (
                "/api/v3/repos/j178/prek/releases/latest".into(),
                Response::ok(release(base, "v0.2.0").to_string()),
            ),
            (
                "/installer".into(),
                // A missing interpreter fails before any installer code runs.
                Response::ok("#!/missing-prek-test-interpreter\n"),
            ),
        ])
    })?;
    let mut command = command(&context, &executable, &server);
    if cfg!(windows) {
        context.write_file("powershell.exe", "not an executable");
        command.env("PATH", context.work_dir().as_ref());
    }
    cmd_snapshot!(context, command, @"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    info: Checking for updates...
    error: Failed to execute the prek installer
    ");
    assert_eq!(server.finish()?.len(), 2);
    check_executable(&executable)?;
    Ok(())
}

/// A stale receipt for another installation must be rejected before any network request.
#[test]
fn rejects_another_installations_receipt() -> Result<()> {
    let context = TestEnv::new();
    let executable = installation(&context, "0.1.0")?;
    let mut receipt: Value = serde_json::from_str(&context.read("prek-receipt.json"))?;
    receipt["install_prefix"] = json!(context.work_dir().join("other-install"));
    context.write_file("prek-receipt.json", serde_json::to_vec(&receipt)?);
    let server = Server::new(|_| HashMap::new())?;
    cmd_snapshot!(context, command(&context, &executable, &server), @"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Self-update is only available for prek binaries installed via the standalone installation scripts.
    hint: If you installed prek with pip, brew, or another package manager, update prek with `pip install --upgrade`, `brew upgrade`, or similar.
    ");
    assert_eq!(server.finish()?, Vec::<String>::new());
    Ok(())
}

/// Latest never downgrades; an equal explicit tag must not invoke the installer either.
#[test]
fn skips_unnecessary_installs() -> Result<()> {
    let context = TestEnv::new().with_filter(env!("CARGO_PKG_VERSION"), "[VERSION]");
    let executable = installation(&context, "999.0.0")?;
    let server = Server::new(|base| {
        HashMap::from([
            (
                "/api/v3/repos/j178/prek/releases/latest".into(),
                Response::ok(release(base, "v0.2.0").to_string()),
            ),
            (
                "/api/v3/repos/j178/prek/releases/tags/v999.0.0".into(),
                Response::ok(release(base, "v999.0.0").to_string()),
            ),
        ])
    })?;
    insta::allow_duplicates! {
    for tag in [None, Some("v999.0.0")] {
        let mut command = command(&context, &executable, &server);
        if let Some(tag) = tag {
            command.arg(tag);
        }
        cmd_snapshot!(context, command, @"
        success: true
        exit_code: 0
        ----- stdout -----

        ----- stderr -----
        info: Checking for updates...
        success: You're on the latest version of prek (v[VERSION])
        ");
    }
    }
    assert_eq!(server.finish()?.len(), 2);
    Ok(())
}
