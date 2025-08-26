use assert_fs::assert::PathAssert;
use assert_fs::fixture::{FileWriteStr, PathChild};

use crate::common::{TestContext, cmd_snapshot};

/// Test `language_version` parsing.
/// Python 3.12.11 is installed in the CI environment, when running tests uv can find them.
/// Other versions may need to be downloaded while running the tests.
#[test]
fn language_version() -> anyhow::Result<()> {
    let context = TestContext::new();
    context.init_project();
    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: python3
                name: python3
                language: python
                entry: python -c 'print("Hello, World!")'
                language_version: python3
                always_run: true
              - id: python3.12
                name: python3.12
                language: python
                entry: python -c 'import sys; print(sys.version_info[:3])'
                language_version: python3.12
                always_run: true
              - id: python3.12
                name: python3.12
                language: python
                entry: python -c 'import sys; print(sys.version_info[:3])'
                language_version: '3.12'
                always_run: true
              - id: python3.12
                name: python3.12
                language: python
                entry: python -c 'import sys; print(sys.version_info[:3])'
                language_version: 'python312'
              - id: python3.12
                name: python3.12
                language: python
                entry: python -c 'import sys; print(sys.version_info[:3])'
                language_version: '312'
                always_run: true
              - id: python3.12
                name: python3.12
                language: python
                entry: python -c 'import sys; print(sys.version_info[:3])'
                language_version: python3.12
                always_run: true
              - id: python3.12
                name: python3.12
                language: python
                entry: python -c 'import sys; print(sys.version_info[:3])'
                language_version: '3.12.1' # will auto download
                always_run: true
    "#});
    context.git_add(".");

    context
        .home_dir()
        .child("tools")
        .child("python")
        .assert(predicates::path::missing());

    cmd_snapshot!(context.filters(), context.run().arg("-v"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    python3..................................................................Passed
    - hook id: python3
    - duration: [TIME]
      Hello, World!
    python3.12...............................................................Passed
    - hook id: python3.12
    - duration: [TIME]
      (3, 12, 11)
    python3.12...............................................................Passed
    - hook id: python3.12
    - duration: [TIME]
      (3, 12, 11)
    python3.12...............................................................Passed
    - hook id: python3.12
    - duration: [TIME]
      (3, 12, 11)
    python3.12...............................................................Passed
    - hook id: python3.12
    - duration: [TIME]
      (3, 12, 11)
    python3.12...............................................................Passed
    - hook id: python3.12
    - duration: [TIME]
      (3, 12, 11)
    python3.12...............................................................Passed
    - hook id: python3.12
    - duration: [TIME]
      (3, 12, 1)

    ----- stderr -----
    "#);

    let tools_python_dir = context.home_dir().join("tools").join("python");

    // Check that either no downloads were needed (directory doesn't exist)
    // or some downloads happened (directory exists with content)
    if tools_python_dir.exists() {
        let count = tools_python_dir
            .read_dir()?
            .flatten()
            .filter(|d| !d.file_name().to_string_lossy().starts_with('.'))
            .count();
        assert!(count > 0, "tools/python directory exists but is empty");
    }

    Ok(())
}

#[test]
fn invalid_version() {
    let context = TestContext::new();
    context.init_project();
    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: local
                name: local
                language: python
                entry: python -c 'print("Hello, world!")'
                language_version: 'invalid-version' # invalid version
                always_run: true
                verbose: true
                pass_filenames: false
    "#});

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Hook `local` is invalid
      caused by: Invalid `language_version` value: `invalid-version`
    "#);
}

/// Request a version that neither can be found nor downloaded.
#[test]
fn can_not_download() {
    let context = TestContext::new();
    context.init_project();
    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: less-than-3.6
                name: less-than-3.6
                language: python
                entry: python -c 'import sys; print(sys.version_info[:3])'
                language_version: '<=3.6' # not supported version
                always_run: true
    "});
    context.git_add(".");

    let mut filters = context
        .filters()
        .into_iter()
        .chain([(
            "managed installations, search path, or registry",
            "managed installations or search path",
        )])
        .collect::<Vec<_>>();
    if cfg!(windows) {
        // Unix uses "exit status", Windows uses "exit code"
        filters.push((r"exit code: ", "exit status: "));
    }

    cmd_snapshot!(filters, context.run().arg("-v"), @r#"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Failed to install hook `less-than-3.6`
      caused by: Failed to create Python virtual environment
      caused by: command `create venv` exited with an error:

    [status]
    exit status: 2

    [stderr]
    error: No interpreter found for Python <=3.6 in managed installations or search path
    "#);
}

/// Test that `additional_dependencies` are installed correctly.
#[test]
fn additional_dependencies() {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: local
                name: local
                language: python
                language_version: '3.11' # will auto download
                entry: pyecho Hello, world!
                additional_dependencies: ["pyecho-cli"]
                always_run: true
                verbose: true
                pass_filenames: false
    "#});

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    local....................................................................Passed
    - hook id: local
    - duration: [TIME]
      Hello, world!

    ----- stderr -----
    "#);
}

/// Ensure that stderr from hooks is captured and shown to the user.
#[test]
fn hook_stderr() -> anyhow::Result<()> {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: local
                name: local
                language: python
                entry: python ./hook.py
    "});

    context
        .work_dir()
        .child("hook.py")
        .write_str("import sys; print('How are you', file=sys.stderr); sys.exit(1)")?;

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    local....................................................................Failed
    - hook id: local
    - exit code: 1
      How are you

    ----- stderr -----
    "#);

    Ok(())
}

/// Test that pep723 script for local hook is installed correctly.
/// Only if no additional dependencies are specified.
#[test]
fn pep723_script() -> anyhow::Result<()> {
    let context = TestContext::new();
    context.init_project();
    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: other-hook
                name: other-hook
                language: python
                entry: python -c 'print("hello from other-hook")'
                verbose: true
                pass_filenames: false
              - id: local
                name: local
                language: python
                entry: ./script.py hello world
                verbose: true
                pass_filenames: false
    "#});
    // On Windows, uv venv does not create `python3.exe`, `python3.12.exe` symlink,
    // be sure to use `python` as the interpreter name.
    context
        .work_dir()
        .child("script.py")
        .write_str(indoc::indoc! {r#"
        #!/usr/bin/env python
        # /// script
        # requires-python = ">=3.10"
        # dependencies = [ "pyecho-cli" ]
        # ///
        from pyecho import main
        main()
    "#})?;

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    other-hook...............................................................Passed
    - hook id: other-hook
    - duration: [TIME]
      hello from other-hook
    local....................................................................Passed
    - hook id: local
    - duration: [TIME]
      hello world

    ----- stderr -----
    "#);

    Ok(())
}
