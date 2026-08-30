use anyhow::Result;
use assert_cmd::Command;
use prek_consts::env_vars::EnvVars;
use prek_consts::prepend_paths;

use crate::common::{TestEnv, cmd_snapshot};

#[test]
fn docker_image() {
    // Test suite from https://github.com/super-linter/super-linter/tree/main/test/linters/gitleaks/bad
    let context = TestEnv::new()
        .with_file(
            "gitleaks_bad_01.txt",
            indoc::indoc! {r"
        aws_access_key_id = AROA47DSWDEZA3RQASWB
        aws_secret_access_key = wQwdsZDiWg4UA5ngO0OSI2TkM4kkYxF6d2S1aYWM
    "},
        )
        .init_git();

    // Use fully qualified image name for Podman/Docker compatibility
    Command::new("docker")
        .args(["pull", "docker.io/zricethezav/gitleaks:v8.21.2"])
        .assert()
        .success();

    // Gitleaks writes findings to stdout and its banner/status logs to stderr.
    // Suppress the latter because Docker does not guarantee their relative order.
    context.write_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: gitleaks-docker
                name: Detect hardcoded secrets
                language: docker_image
                entry: docker.io/zricethezav/gitleaks:v8.21.2 git --pre-commit --redact --staged --verbose --no-banner --log-level=error
                pass_filenames: false
    "});
    context.git().add(".");

    cmd_snapshot!(context, context.run(), @r#"
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

    ----- stderr -----
    "#);
}

/// Test that `docker_image` does not try to resolve entry in the host system PATH.
#[test]
fn docker_image_does_not_resolve_entry() -> Result<()> {
    let context = TestEnv::new()
        .with_executable_file("bin/alpine", "#!/bin/sh\necho host\n")
        .init_git();

    let bin_dir = context.child("bin");

    Command::new("docker")
        .args(["pull", "docker.io/library/alpine:latest"])
        .assert()
        .success();

    context.write_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: alpine-echo
                name: Alpine echo
                language: docker_image
                entry: alpine /bin/sh -c 'echo ok'
                pass_filenames: false
                always_run: true
                verbose: true
    "});
    context.git().add(".");

    let mut cmd = context.run();
    cmd.env(EnvVars::PATH, prepend_paths(&[bin_dir.path()])?);

    cmd_snapshot!(context, cmd, @r"
    success: true
    exit_code: 0
    ----- stdout -----
    Alpine echo..............................................................Passed
    - hook id: alpine-echo
    - duration: [TIME]

      ok

    ----- stderr -----
    ");

    Ok(())
}
