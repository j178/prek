use prek_consts::PRE_COMMIT_HOOKS_YAML;

use crate::common::{TestEnv, cmd_snapshot};

/// Test that a local Swift hook with a system command works.
#[test]
fn local_hook_system_command() {
    let context = TestEnv::new_git().with_config(indoc::indoc! {r#"
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

    context.git().add_all();

    cmd_snapshot!(context, context.run(), @r"
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
    let context = TestEnv::new_git().with_config(indoc::indoc! {r"
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

    context.git().add_all();

    cmd_snapshot!(context, context.run(), @r"
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
    let context = TestEnv::new_git().with_config(indoc::indoc! {r#"
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

    context.git().add_all();

    // First run - installs
    cmd_snapshot!(context, context.run(), @r"
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
    cmd_snapshot!(context, context.run(), @r"
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
fn local_package_build() {
    // Create a minimal Swift package
    let swift_hook = TestEnv::new_git()
        .with_file(
            "Package.swift",
            indoc::indoc! {r#"
                // swift-tools-version:6.0
                import PackageDescription

                let package = Package(
                    name: "prek-swift-test",
                    targets: [
                        .executableTarget(name: "prek-swift-test", path: "Sources")
                    ]
                )
            "#},
        )
        .with_file(
            "Sources/main.swift",
            indoc::indoc! {r#"
                print("Hello from Swift package!")
            "#},
        )
        .with_file(
            PRE_COMMIT_HOOKS_YAML,
            indoc::indoc! {r"
                - id: swift-package-test
                  name: swift-package-test
                  entry: prek-swift-test
                  language: swift
            "},
        );
    swift_hook
        .git()
        .add_all()
        .commit("Initial commit")
        .tag("v1.0");

    let context = TestEnv::new_git();

    let hook_url = swift_hook.work_dir().to_str().unwrap();
    context.write_config(indoc::formatdoc! {r"
        repos:
          - repo: {hook_url}
            rev: v1.0
            hooks:
              - id: swift-package-test
                verbose: true
                always_run: true
                pass_filenames: false
    ", hook_url = hook_url});
    context.git().add_all();

    cmd_snapshot!(context, context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    swift-package-test.......................................................Passed
    - hook id: swift-package-test
    - duration: [TIME]

      Hello from Swift package!

    ----- stderr -----
    ");
}
