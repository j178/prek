use assert_fs::fixture::{FileWriteStr, PathChild, PathCreateDir};
use prek_consts::MANIFEST_FILE;
use prek_consts::env_vars::EnvVars;

use crate::common::{TestContext, cmd_snapshot, git_cmd};

/// Test that a local Swift hook with a system command works.
#[test]
fn local_hook_system_command() {
    if !EnvVars::is_set(EnvVars::CI) {
        return;
    }

    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: echo-swift
                name: echo-swift
                language: swift
                entry: echo "Swift hook ran"
                always_run: true
                verbose: true
                pass_filenames: false
    "#});

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    echo-swift...............................................................Passed
    - hook id: echo-swift
    - duration: [TIME]

      Swift hook ran

    ----- stderr -----
    ");
}

/// Test that `language_version` is rejected for Swift.
#[test]
fn language_version_rejected() {
    if !EnvVars::is_set(EnvVars::CI) {
        return;
    }

    let context = TestContext::new();
    context.init_project();
    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: local
                name: local
                language: swift
                entry: swift --version
                language_version: '6.0'
                always_run: true
                pass_filenames: false
    "});

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Failed to init hooks
      caused by: Invalid hook `local`
      caused by: Hook specified `language_version: 6.0` but the language `swift` does not support toolchain installation for now
    ");
}

/// Test that health check works after install.
#[test]
fn health_check() {
    if !EnvVars::is_set(EnvVars::CI) {
        return;
    }

    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: swift-echo
                name: swift-echo
                language: swift
                entry: echo "Hello"
                always_run: true
                verbose: true
                pass_filenames: false
    "#});

    context.git_add(".");

    // First run - installs
    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    swift-echo...............................................................Passed
    - hook id: swift-echo
    - duration: [TIME]

      Hello

    ----- stderr -----
    ");

    // Second run - health check
    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    swift-echo...............................................................Passed
    - hook id: swift-echo
    - duration: [TIME]

      Hello

    ----- stderr -----
    ");
}

/// Test that a Swift Package.swift is built and the executable is available.
#[test]
fn local_package_build() -> anyhow::Result<()> {
    if !EnvVars::is_set(EnvVars::CI) {
        return Ok(());
    }

    let swift_hook = TestContext::new();
    swift_hook.init_project();

    // Create a minimal Swift package
    swift_hook
        .work_dir()
        .child("Package.swift")
        .write_str(indoc::indoc! {r#"
        // swift-tools-version:6.0
        import PackageDescription

        let package = Package(
            name: "prek-swift-test",
            targets: [
                .executableTarget(name: "prek-swift-test", path: "Sources")
            ]
        )
    "#})?;
    swift_hook.work_dir().child("Sources").create_dir_all()?;
    swift_hook
        .work_dir()
        .child("Sources/main.swift")
        .write_str(indoc::indoc! {r#"
        print("Hello from Swift package!")
    "#})?;
    swift_hook
        .work_dir()
        .child(MANIFEST_FILE)
        .write_str(indoc::indoc! {r"
        - id: swift-package-test
          name: swift-package-test
          entry: prek-swift-test
          language: swift
    "})?;
    swift_hook.git_add(".");
    swift_hook.git_commit("Initial commit");
    git_cmd(swift_hook.work_dir())
        .args(["tag", "v1.0", "-m", "v1.0"])
        .output()?;

    let context = TestContext::new();
    context.init_project();

    let hook_url = swift_hook.work_dir().to_str().unwrap();
    context.write_pre_commit_config(&indoc::formatdoc! {r"
        repos:
          - repo: {hook_url}
            rev: v1.0
            hooks:
              - id: swift-package-test
                verbose: true
                always_run: true
                pass_filenames: false
    ", hook_url = hook_url});
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    swift-package-test.......................................................Passed
    - hook id: swift-package-test
    - duration: [TIME]

      Hello from Swift package!

    ----- stderr -----
    ");

    Ok(())
}
