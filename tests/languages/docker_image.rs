use anyhow::{Result, bail};
use assert_cmd::Command;
use assert_fs::fixture::{FileWriteStr, PathChild};
use prek_consts::env_vars::EnvVars;

use crate::common::{TestContext, cmd_snapshot};

fn detect_container_runtime() -> Result<String> {
    let podman = which::which("podman");
    let docker = which::which("docker");

    if let Some(val) = EnvVars::var_os(EnvVars::PREK_CONTAINER_RUNTIME)
        && let Some(val) = val.to_ascii_lowercase().to_str()
    {
        if val == "docker" || val == "podman" {
            return Ok(val.to_owned());
        }
    }

    if let Ok(_p) = docker {
        return Ok("docker".to_owned());
    } else if let Ok(_p) = podman {
        return Ok("podman".to_owned());
    }

    bail!("No container runtime detected");
}

#[test]
fn docker_image() -> Result<()> {
    let context = TestContext::new();
    context.init_project();

    let cwd = context.work_dir();
    // Test suite from https://github.com/super-linter/super-linter/tree/main/test/linters/gitleaks/bad
    cwd.child("gitleaks_bad_01.txt")
        .write_str(indoc::indoc! {r"
        aws_access_key_id = AROA47DSWDEZA3RQASWB
        aws_secret_access_key = wQwdsZDiWg4UA5ngO0OSI2TkM4kkYxF6d2S1aYWM
    "})?;

    // Use fully qualified image name for Podman/Docker compatibility
    Command::new(detect_container_runtime()?)
        .args(["pull", "docker.io/zricethezav/gitleaks:v8.21.2"])
        .assert()
        .success();

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: gitleaks-docker
                name: Detect hardcoded secrets
                language: docker_image
                entry: docker.io/zricethezav/gitleaks:v8.21.2 git --pre-commit --redact --staged --verbose
                pass_filenames: false
    "});
    context.git_add(".");

    let filters = context
        .filters()
        .into_iter()
        .chain([(r"\d\d?:\d\d(AM|PM)", "[TIME]")])
        .collect::<Vec<_>>();

    cmd_snapshot!(filters, context.run(), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    Detect hardcoded secrets.................................................Failed
    - hook id: gitleaks-docker
    - exit code: 1
      Finding:     aws_access_key_id = REDACTED
      Secret:      REDACTED
      RuleID:      generic-api-key
      Entropy:     3.521928
      File:        gitleaks_bad_01.txt
      Line:        1
      Fingerprint: gitleaks_bad_01.txt:generic-api-key:1

      Finding:     aws_secret_access_key = REDACTED
      Secret:      REDACTED
      RuleID:      generic-api-key
      Entropy:     4.703056
      File:        gitleaks_bad_01.txt
      Line:        2
      Fingerprint: gitleaks_bad_01.txt:generic-api-key:2


          ○
          │╲
          │ ○
          ○ ░
          ░    gitleaks

      [TIME] INF 1 commits scanned.
      [TIME] INF scan completed in [TIME]
      [TIME] WRN leaks found: 2

    ----- stderr -----
    "#);
    Ok(())
}
