mod common;

use crate::common::{TestContext, cmd_snapshot, git_cmd};

use assert_cmd::assert::OutputAssertExt;
use assert_fs::fixture::{FileWriteStr, PathChild, PathCreateDir};
use prek_consts::env_vars::EnvVars;

#[test]
fn self_repo_system_hook() -> anyhow::Result<()> {
    let context = TestContext::new();
    context.init_project();

    let cwd = context.work_dir();

    cwd.child(".pre-commit-hooks.yaml")
        .write_str("- id: echo-hook\n  name: echo hook\n  entry: echo\n  language: system\n  files: \"\\\\.(txt|md)$\"\n")?;

    cwd.child("hello.txt").write_str("Hello\n")?;
    cwd.child("ignored.rs").write_str("fn main() {}\n")?;

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: self
            hooks:
              - id: echo-hook
    "});
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    echo hook................................................................Passed

    ----- stderr -----
    ");

    Ok(())
}

#[test]
fn self_repo_with_overrides() -> anyhow::Result<()> {
    let context = TestContext::new();
    context.init_project();

    let cwd = context.work_dir();

    cwd.child(".pre-commit-hooks.yaml")
        .write_str(indoc::indoc! {r"
    - id: echo-hook
      name: echo hook
      entry: echo
      language: system
    "})?;

    cwd.child("hello.txt").write_str("Hello\n")?;

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: self
            hooks:
              - id: echo-hook
                name: overridden name
                args: [--verbose]
    "});
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    overridden name..........................................................Passed

    ----- stderr -----
    ");

    Ok(())
}

#[test]
fn self_repo_missing_manifest() {
    let context = TestContext::new();
    context.init_project();

    context
        .work_dir()
        .child("hello.txt")
        .write_str("Hello\n")
        .unwrap();

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: self
            hooks:
              - id: my-hook
    "});
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Failed to init hooks
      caused by: Failed to read manifest of `self`
      caused by: failed to open file `[TEMP_DIR]/.pre-commit-hooks.yaml`: No such file or directory (os error 2)
    ");
}

#[test]
fn self_repo_unknown_hook_id() -> anyhow::Result<()> {
    let context = TestContext::new();
    context.init_project();

    let cwd = context.work_dir();

    cwd.child(".pre-commit-hooks.yaml")
        .write_str(indoc::indoc! {r"
    - id: real-hook
      name: real hook
      entry: echo
      language: system
    "})?;

    cwd.child("hello.txt").write_str("Hello\n")?;

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: self
            hooks:
              - id: nonexistent-hook
    "});
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Failed to init hooks
      caused by: Hook `nonexistent-hook` not present in repo `self`
    ");

    Ok(())
}

/// Two projects sharing the same `PREK_HOME` must not reuse each other's
/// self-repo hook environments. Each project installs its own package
/// into the env, so sharing would cause incorrect behavior.
#[test]
fn self_repo_cross_project_env_isolation() -> anyhow::Result<()> {
    // Use one TestContext for the shared PREK_HOME.
    let context = TestContext::new();
    let home = context.home_dir();

    // Set up two independent projects under the context root.
    let project_a = context.work_dir().child("project_a");
    let project_b = context.work_dir().child("project_b");
    project_a.create_dir_all()?;
    project_b.create_dir_all()?;

    for (project, label) in [(&project_a, "a"), (&project_b, "b")] {
        git_cmd(project)
            .arg("-c")
            .arg("init.defaultBranch=master")
            .arg("init")
            .assert()
            .success();

        project.child(".pre-commit-hooks.yaml").write_str(&format!(
            indoc::indoc! {r#"
                - id: greet
                  name: greet
                  entry: python -c "print('from-{label}')"
                  language: python
            "#},
            label = label,
        ))?;

        project.child("setup.py").write_str(&format!(
            "from setuptools import setup; setup(name='project-{label}', version='0.0.1')",
        ))?;

        project
            .child(".pre-commit-config.yaml")
            .write_str(indoc::indoc! {r"
                repos:
                  - repo: self
                    hooks:
                      - id: greet
            "})?;

        project.child("file.txt").write_str("Hello\n")?;

        git_cmd(project).args(["add", "."]).assert().success();
    }

    let prek_bin = EnvVars::var_os("NEXTEST_BIN_EXE_prek")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from(assert_cmd::cargo::cargo_bin!("prek")));

    // Run project A — installs its env.
    let output_a = std::process::Command::new(&prek_bin)
        .arg("run")
        .current_dir(&*project_a)
        .env(EnvVars::PREK_HOME, &**home)
        .env(EnvVars::PREK_INTERNAL__SORT_FILENAMES, "1")
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "core.autocrlf")
        .env("GIT_CONFIG_VALUE_0", "false")
        .output()?;

    let stdout_a = String::from_utf8_lossy(&output_a.stdout);
    assert!(
        output_a.status.success(),
        "project A failed:\nstdout: {stdout_a}\nstderr: {}",
        String::from_utf8_lossy(&output_a.stderr),
    );
    assert!(stdout_a.contains("Passed"), "project A hook did not pass");

    // Run project B — must create a separate env, not reuse A's.
    let output_b = std::process::Command::new(&prek_bin)
        .arg("run")
        .current_dir(&*project_b)
        .env(EnvVars::PREK_HOME, &**home)
        .env(EnvVars::PREK_INTERNAL__SORT_FILENAMES, "1")
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "core.autocrlf")
        .env("GIT_CONFIG_VALUE_0", "false")
        .output()?;

    let stdout_b = String::from_utf8_lossy(&output_b.stdout);
    assert!(
        output_b.status.success(),
        "project B failed:\nstdout: {stdout_b}\nstderr: {}",
        String::from_utf8_lossy(&output_b.stderr),
    );
    assert!(stdout_b.contains("Passed"), "project B hook did not pass");

    // Verify the hook envs are distinct (at least 2 python-* dirs under hooks/).
    let hooks_dir = home.child("hooks");
    let python_envs: Vec<_> = fs_err::read_dir(hooks_dir.path())?
        .filter_map(Result::ok)
        .filter(|e| {
            e.file_type().map(|t| t.is_dir()).unwrap_or(false)
                && e.file_name().to_string_lossy().starts_with("python-")
        })
        .collect();

    assert!(
        python_envs.len() >= 2,
        "expected at least 2 separate python hook envs, found {}",
        python_envs.len(),
    );

    Ok(())
}
