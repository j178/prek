use assert_fs::assert::PathAssert;
use assert_fs::fixture::{FileWriteStr, PathChild};
use prek_consts::PRE_COMMIT_HOOKS_YAML;
use prek_consts::env_vars::{EnvVars, EnvVarsRead};

use crate::common::{TestEnv, cmd_snapshot};

/// Test `language_version` parsing and downloading.
/// We use `setup-python` action to install Python 3.12 in CI, when running tests uv can find them.
/// Other versions may need to be downloaded while running the tests.
#[test]
fn language_version() -> anyhow::Result<()> {
    if !EnvVars.is_set(EnvVars::CI) {
        // Skip when not running in CI, as we may have other Python versions installed locally.
        return Ok(());
    }

    let context = TestEnv::new_git().with_config(indoc::indoc! {r#"
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
                entry: python -c 'import sys; print(sys.version_info[:2])'
                language_version: python3.12
                always_run: true
              - id: python3.12
                name: python3.12
                language: python
                entry: python -c 'import sys; print(sys.version_info[:2])'
                language_version: '3.12'
                always_run: true
              - id: python3.12
                name: python3.12
                language: python
                entry: python -c 'import sys; print(sys.version_info[:2])'
                language_version: 'python312'
              - id: python3.12
                name: python3.12
                language: python
                entry: python -c 'import sys; print(sys.version_info[:2])'
                language_version: '312'
                always_run: true
              - id: python3.12
                name: python3.12
                language: python
                entry: python -c 'import sys; print(sys.version_info[:2])'
                language_version: python3.12
                always_run: true
              - id: python3.12
                name: python3.12
                language: python
                entry: python -c 'import sys; print(sys.version_info[:2])'
                language_version: '3.11.1' # will auto download
                always_run: true
    "#});
    context.git().add_all();

    let python_dir = context.home_dir().child("tools").child("python");
    python_dir.assert(predicates::path::missing());

    cmd_snapshot!(context, context.run().arg("-v"), @r#"
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

      (3, 12)
    python3.12...............................................................Passed
    - hook id: python3.12
    - duration: [TIME]

      (3, 12)
    python3.12...............................................................Passed
    - hook id: python3.12
    - duration: [TIME]

      (3, 12)
    python3.12...............................................................Passed
    - hook id: python3.12
    - duration: [TIME]

      (3, 12)
    python3.12...............................................................Passed
    - hook id: python3.12
    - duration: [TIME]

      (3, 12)
    python3.12...............................................................Passed
    - hook id: python3.12
    - duration: [TIME]

      (3, 11)

    ----- stderr -----
    "#);

    // Check that only Python 3.11 is installed.
    let installed_versions = python_dir
        .read_dir()?
        .flatten()
        .filter_map(|d| {
            if d.file_type().ok()?.is_symlink() {
                // Skip symlinks, which may point to other versions.
                return None;
            }
            let filename = d.file_name().to_string_lossy().into_owned();
            if filename.starts_with('.') {
                None
            } else {
                Some(filename)
            }
        })
        .collect::<Vec<_>>();

    assert_eq!(
        installed_versions.len(),
        1,
        "Expected only one Python version to be installed, but found: {installed_versions:?}"
    );
    assert!(
        installed_versions.iter().any(|v| v.contains("3.11")),
        "Expected Python 3.11 to be installed, but found: {installed_versions:?}"
    );

    Ok(())
}

#[test]
fn invalid_version() {
    let context = TestEnv::new_git().with_config(indoc::indoc! {r#"
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

    context.git().add_all();

    cmd_snapshot!(context, context.run(), @r"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Failed to init hooks
      caused by: Invalid hook `local`
      caused by: Invalid `language_version` value: `invalid-version`
    ");
}

/// Request a version that neither can be found nor downloaded.
#[test]
fn can_not_download() {
    let context = TestEnv::new_git().with_config(indoc::indoc! {r"
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
    context.git().add_all();

    let context = context.with_filters([
        (
            "managed installations, search path, or registry",
            "managed installations or search path",
        ),
        (r"Command `[^`]*uv(?:\.exe)? venv", "Command `[UV] venv"),
        (r"python-[[:alnum:]]{20}", "python-[HASH]"),
    ]);

    cmd_snapshot!(
        context,
        context
            .run()
            .arg("-v")
            .env(EnvVars::UV_PYTHON_PREFERENCE, "managed"), @r#"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Failed to install hook `less-than-3.6`
      caused by: Failed to create Python virtual environment
      caused by: Command `[UV] venv [HOME]/hooks/python-[HASH]` exited with an error:

    [status]
    exit status: 2

    [stderr]
    error: No interpreter found for Python <=3.6 in managed installations or search path
    "#);
}

/// Test that `additional_dependencies` are installed correctly.
#[test]
fn additional_dependencies() {
    let context = TestEnv::new_git().with_config(indoc::indoc! {r#"
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

    context.git().add_all();

    cmd_snapshot!(context, context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    local....................................................................Passed
    - hook id: local
    - duration: [TIME]

      Hello, world!

    ----- stderr -----
    ");
}

#[test]
fn additional_dependencies_in_remote_repo() -> anyhow::Result<()> {
    let context = TestEnv::new_git();
    let repo = context.create_repo("python-hook");
    let repo_path = repo.path();
    repo_path
        .child(PRE_COMMIT_HOOKS_YAML)
        .write_str(indoc::indoc! {r#"
        - id: hello
          name: hello
          language: python
          entry: pyecho Greetings from hook
          additional_dependencies: [".[cli]"]
    "#})?;
    repo_path.child("module.py").write_str(indoc::indoc! {r#"
        def greet():
            print("Greetings from module")
    "#})?;
    repo_path.child("setup.py").write_str(indoc::indoc! {r#"
        from setuptools import setup, find_packages

        setup(
            name="remote-hooks",
            version="0.1.0",
            py_modules=["module"],
            extras_require={
                "cli": ["pyecho-cli"]
            }
        )
    "#})?;
    repo.git().add_all().commit("Add manifest").tag("v0.1.0");

    let context = context.with_config(indoc::formatdoc! {r"
        repos:
          - repo: {}
            rev: v0.1.0
            hooks:
              - id: hello
                name: hello
                verbose: true
    ", repo_path.display()});

    context.git().add_all();
    cmd_snapshot!(context, context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    hello....................................................................Passed
    - hook id: hello
    - duration: [TIME]

      Greetings from hook .pre-commit-config.yaml

    ----- stderr -----
    ");

    Ok(())
}

/// Ensure that stderr from hooks is captured and shown to the user.
#[test]
fn hook_stderr() -> anyhow::Result<()> {
    let context = TestEnv::new_git().with_config(indoc::indoc! {r"
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

    context.git().add_all();

    cmd_snapshot!(context, context.run(), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    local....................................................................Failed
    - hook id: local
    - exit code: 1

      How are you

    ----- stderr -----
    ");

    Ok(())
}

/// Test that pep723 script for local hook is installed correctly.
/// Only if no additional dependencies are specified.
#[test]
fn pep723_script() -> anyhow::Result<()> {
    let context = TestEnv::new_git().with_config(indoc::indoc! {r#"
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

    context.git().add_all();

    cmd_snapshot!(context, context.run(), @r"
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
    ");

    Ok(())
}

/// Test that GIT environment variables do not leak into uv pip install subprocess.
/// When prek runs in a git worktree, git sets `GIT_DIR` which should not propagate to
/// pip install where it breaks packages using `setuptools_scm` for file discovery.
///
/// Regression test for <https://github.com/j178/prek/issues/1354>
#[test]
fn git_env_vars_not_leaked_to_pip_install() -> anyhow::Result<()> {
    let context = TestEnv::new_git();

    // setup.py that fails if GIT_DIR leaks into pip install
    context
        .work_dir()
        .child("setup.py")
        .write_str(indoc::indoc! {r#"
        import os, sys
        from setuptools import setup
        if os.environ.get("GIT_DIR"):
            sys.exit("ERROR: GIT_DIR should not leak into pip install")
        setup(name="test", version="0.1.0", extras_require={"test": []})
    "#})?;

    let dependency = serde_json::to_string(&format!(
        "{}[test]",
        context.work_dir().path().to_string_lossy()
    ))?;
    let context = context.with_config(indoc::formatdoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: check-no-git-dir
                name: check-no-git-dir
                language: python
                entry: python -c "print('ok')"
                additional_dependencies: [{dependency}]
                always_run: true
    "#});

    context.git().add_all();

    // Simulate worktree environment by setting GIT_DIR (like git does in worktrees)
    cmd_snapshot!(context, context.run()
        .env("GIT_DIR", context.work_dir().join(".git")), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    check-no-git-dir.........................................................Passed

    ----- stderr -----
    ");

    Ok(())
}

/// Regression test for <https://github.com/j178/prek/issues/1603>.
#[test]
fn local_relative_additional_dependency_is_not_resolved_from_worktree() -> anyhow::Result<()> {
    let context = TestEnv::new_git();
    context
        .work_dir()
        .child("pyproject.toml")
        .write_str(indoc::indoc! {r#"
            [project]
            name = "local-project"
            version = "0.1.0"
        "#})?;

    let context = context
        .with_config(indoc::indoc! {r#"
            repos:
              - repo: local
                hooks:
                  - id: local-project
                    name: local-project
                    language: python
                    entry: python -c "print('ok')"
                    additional_dependencies: ["."]
                    always_run: true
        "#})
        .with_filters([
            (r"Command `[^`]*uv(?:\.exe)? pip", "Command `[UV] pip"),
            (r"python-[[:alnum:]]{20}", "python-[HASH]"),
            (
                r"error: .*\.tmp[[:alnum:]]+ does not appear",
                "error: [INSTALL_CWD] does not appear",
            ),
            (
                r"Using Python [^ ]+ environment",
                "Using Python [VERSION] environment",
            ),
        ]);

    context.git().add_all();

    cmd_snapshot!(context, context.run(), @r#"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Failed to install hook `local-project`
      caused by: Command `[UV] pip install --project / .` exited with an error:

    [status]
    exit status: 2

    [stderr]
    Using Python [VERSION] environment at: [HOME]/hooks/python-[HASH]
    error: [INSTALL_CWD] does not appear to be a Python project, as neither `pyproject.toml` nor `setup.py` are present in the directory
    "#);

    Ok(())
}

/// Test that health check passes when Python toolchain path involves symlinks.
/// The stored toolchain path and the queried path should be canonicalized before comparison.
///
/// Regression test for symlink-related "Python executable mismatch" errors.
#[test]
#[cfg(unix)]
fn health_check_with_symlinked_toolchain() -> anyhow::Result<()> {
    use fs_err::os::unix::fs::symlink;
    use prek_consts::prepend_paths;

    let context = TestEnv::new_git();

    // Find a Python executable, create a symlinked directory to its parent,
    // and prepend that to PATH so that prek picks up the symlinked path.
    let python_executable = which::which("python3")?;
    let symlinked_bin = context.work_dir().child("symlinked-bin");
    symlink(python_executable.parent().unwrap(), &symlinked_bin)?;
    let new_path = prepend_paths(&[&*symlinked_bin])?;

    let context = context.with_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: local
                name: local
                language: python
                entry: python -c 'print("hello")'
                always_run: true
                pass_filenames: false
    "#});
    context.git().add_all();

    // First run installs the hook
    cmd_snapshot!(context, context.run().env(EnvVars::PATH, new_path), @"
    success: true
    exit_code: 0
    ----- stdout -----
    local....................................................................Passed

    ----- stderr -----
    ");

    let hooks_dir = context.home_dir().child("hooks");
    let hook_envs = hooks_dir
        .read_dir()?
        .flatten()
        .filter(|d| d.file_name().to_string_lossy().starts_with("python-"))
        .collect::<Vec<_>>();
    assert_eq!(
        hook_envs.len(),
        1,
        "Expected one installed hook env, found: {hook_envs:?}",
    );

    // Second run triggers health check with a symlinked toolchain path
    cmd_snapshot!(context, context.run(), @"
    success: true
    exit_code: 0
    ----- stdout -----
    local....................................................................Passed

    ----- stderr -----
    ");

    let hook_envs = hooks_dir
        .read_dir()?
        .flatten()
        .filter(|d| d.file_name().to_string_lossy().starts_with("python-"))
        .collect::<Vec<_>>();
    assert_eq!(
        hook_envs.len(),
        1,
        "Expected one installed hook env, found: {hook_envs:?}",
    );

    Ok(())
}
