use std::path::Path;
#[cfg(target_os = "linux")]
use std::{ffi::OsString, os::unix::ffi::OsStringExt};

use anyhow::Result;
use assert_cmd::assert::OutputAssertExt;
use assert_fs::assert::PathAssert;
use assert_fs::fixture::PathChild;
use prek_consts::env_vars::{EnvVars, EnvVarsRead};

use crate::common::{TestContext, cmd_snapshot, git_cmd};

fn visible_entry_names(path: &Path) -> Result<Vec<String>> {
    let mut names = fs_err::read_dir(path)?
        .map(|entry| entry.map(|entry| entry.file_name().to_string_lossy().into_owned()))
        .collect::<std::io::Result<Vec<_>>>()?;
    names.retain(|name| !name.starts_with('.'));
    names.sort();
    Ok(names)
}

#[test]
fn managed_install_and_environment_reuse() -> Result<()> {
    // This exercises release-backed installation in the three-platform language-test matrix.
    if !EnvVars.is_set(EnvVars::CI) {
        return Ok(());
    }

    let context = TestContext::new();
    context.init_project();
    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: mise-version
                name: mise version
                language: mise
                language_version: "=2026.7.18"
                entry: mise --version
                always_run: true
                verbose: true
                pass_filenames: false
              - id: mise-dependency-one
                name: mise dependency one
                language: mise
                language_version: "=2026.7.18"
                entry: zoxide --version
                additional_dependencies: ["github:ajeetdsouza/zoxide@0.10.0"]
                always_run: true
                verbose: true
                pass_filenames: false
              - id: mise-dependency-two
                name: mise dependency two
                language: mise
                language_version: "=2026.7.18"
                entry: zoxide --version
                additional_dependencies: ["github:ajeetdsouza/zoxide@0.10.0"]
                always_run: true
                verbose: true
                pass_filenames: false
    "#});
    // Make any fallback to the calling project's mise state fail loudly.
    fs_err::write(context.work_dir().join("mise.toml"), "not valid = [")?;
    fs_err::write(context.work_dir().join(".miserc.toml"), "not valid = [")?;
    context.git_add(".");

    let mise_dir = context.home_dir().child("tools").child("mise");
    mise_dir.assert(predicates::path::missing());
    let ambient_mise_dir = context.work_dir().join("ambient-mise-data");

    let filters = context
        .filters()
        .into_iter()
        .chain([(r"2026\.7\.18 [^\r\n]+", "2026.7.18 [BUILD]")])
        .collect::<Vec<_>>();

    // Inherited mise state must not alter or receive the hook environment.
    cmd_snapshot!(filters.clone(), context.run()
        .env(EnvVars::MISE_DATA_DIR, &ambient_mise_dir)
        .env("MISE_ENV", "prek")
        .env("__MISE_DIFF", "invalid inherited state"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    mise version.............................................................Passed
    - hook id: mise-version
    - duration: [TIME]

      2026.7.18 [BUILD]
    mise dependency one......................................................Passed
    - hook id: mise-dependency-one
    - duration: [TIME]

      zoxide 0.10.0
    mise dependency two......................................................Passed
    - hook id: mise-dependency-two
    - duration: [TIME]

      zoxide 0.10.0

    ----- stderr -----
    "#);
    assert!(
        !ambient_mise_dir.exists(),
        "Inherited MISE_DATA_DIR must not receive hook tools"
    );

    let installed_versions = visible_entry_names(mise_dir.path())?;
    assert_eq!(
        installed_versions.len(),
        1,
        "Expected one managed mise version, but found: {installed_versions:?}"
    );
    assert!(
        installed_versions
            .iter()
            .any(|version| version.contains("2026.7.18")),
        "Expected mise 2026.7.18 to be installed, but found: {installed_versions:?}"
    );

    let hooks_dir = context.home_dir().child("hooks");
    let mise_environments = visible_entry_names(hooks_dir.path())?
        .into_iter()
        .filter(|name| name.starts_with("mise-"))
        .collect::<Vec<_>>();
    assert_eq!(
        mise_environments.len(),
        2,
        "Hooks with identical requirements should share an environment, while different dependency sets stay isolated: {mise_environments:?}"
    );
    let tool_env_count = mise_environments
        .iter()
        .filter(|name| {
            fs_err::read_dir(
                hooks_dir
                    .child(name.as_str())
                    .child("mise")
                    .child("data")
                    .child("installs")
                    .path(),
            )
            .is_ok_and(|mut entries| entries.next().is_some())
        })
        .count();
    assert_eq!(
        tool_env_count, 1,
        "The installed tool should only exist in the dependency environment"
    );

    cmd_snapshot!(filters.clone(), context.exec().args([
        "mise-dependency-one",
        "--",
        "zoxide",
        "--version",
    ]), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    zoxide 0.10.0

    ----- stderr -----
    ");

    // Cached health checks must not load inherited global mise configuration.
    let ambient_config_dir = context.work_dir().join("ambient-mise-config");
    fs_err::create_dir_all(&ambient_config_dir)?;
    fs_err::write(ambient_config_dir.join("miserc.toml"), "not valid = [")?;
    context
        .run()
        .env(EnvVars::MISE_CONFIG_DIR, ambient_config_dir)
        .assert()
        .success();

    assert_eq!(visible_entry_names(mise_dir.path())?, installed_versions);
    let reused_environments = visible_entry_names(hooks_dir.path())?
        .into_iter()
        .filter(|name| name.starts_with("mise-"))
        .collect::<Vec<_>>();
    assert_eq!(reused_environments, mise_environments);

    let hook_repo = context.home_dir().child("mise-hook-repo");
    fs_err::create_dir_all(&hook_repo)?;
    fs_err::write(
        hook_repo.join(".pre-commit-hooks.yaml"),
        indoc::indoc! {r#"
            - id: remote-mise-config
              name: remote mise config
              language: mise
              language_version: "=2026.7.18"
              entry: zoxide --version
              always_run: true
              verbose: true
              pass_filenames: false
            - id: remote-mise-dependency
              name: remote mise dependency
              language: mise
              language_version: "=2026.7.18"
              entry: zoxide --version
              additional_dependencies: ["github:ajeetdsouza/zoxide@0.10.0"]
              always_run: true
              verbose: true
              pass_filenames: false
        "#},
    )?;
    // The distinct version and template prove that activation came from the remote root.
    fs_err::write(
        hook_repo.join("mise.toml"),
        indoc::indoc! {r#"
            [tools]
            "github:ajeetdsouza/zoxide" = '{{ read_file(path="zoxide-version.txt") | trim }}'
        "#},
    )?;
    fs_err::write(hook_repo.join("zoxide-version.txt"), "0.9.8\n")?;
    fs_err::write(hook_repo.join(".miserc.toml"), "not valid = [")?;
    git_cmd(&hook_repo).arg("init").assert().success();
    git_cmd(&hook_repo).args(["add", "."]).assert().success();
    git_cmd(&hook_repo)
        .args(["commit", "-m", "Add mise hook"])
        .assert()
        .success();
    let rev_output = git_cmd(&hook_repo).args(["rev-parse", "HEAD"]).output()?;
    let rev = String::from_utf8(rev_output.stdout)?;
    context.write_pre_commit_config(&indoc::formatdoc! {r"
        repos:
          - repo: '{}'
            rev: {}
            hooks:
              - id: remote-mise-config
              - id: remote-mise-dependency
    ", hook_repo.display(), rev.trim()});
    context.git_add(".pre-commit-config.yaml");

    cmd_snapshot!(filters.clone(), context.run().arg("remote-mise-config"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    remote mise config.......................................................Passed
    - hook id: remote-mise-config
    - duration: [TIME]

      zoxide 0.9.8

    ----- stderr -----
    ");

    // Explicit dependencies override the repository manifest for this hook.
    cmd_snapshot!(filters.clone(), context.run().arg("remote-mise-dependency"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    remote mise dependency...................................................Passed
    - hook id: remote-mise-dependency
    - duration: [TIME]

      zoxide 0.10.0

    ----- stderr -----
    ");

    #[cfg(target_os = "linux")]
    {
        // mise exports PATH as JSON, but the ambient PATH can still contain non-UTF-8 entries.
        let path_dir = context
            .work_dir()
            .join(OsString::from_vec(b"non-utf8-path-\xff".to_vec()))
            .join("bin");
        fs_err::create_dir_all(&path_dir)?;
        std::os::unix::fs::symlink("/bin/true", path_dir.join("mise-path-command"))?;
        let path = std::env::join_paths(
            std::iter::once(path_dir).chain(
                EnvVars
                    .var_os(EnvVars::PATH)
                    .as_ref()
                    .into_iter()
                    .flat_map(std::env::split_paths),
            ),
        )?;
        context.write_pre_commit_config(indoc::indoc! {r#"
            repos:
              - repo: local
                hooks:
                  - id: mise-path
                    name: mise path
                    language: mise
                    language_version: "=2026.7.18"
                    entry: mise-path-command
                    additional_dependencies: ["github:ajeetdsouza/zoxide@0.10.0"]
                    always_run: true
                    pass_filenames: false
        "#});
        context
            .run()
            .arg("mise-path")
            .env(EnvVars::PATH, path)
            .assert()
            .success();

        // Hook filenames must not pass through mise's UTF-8 environment exchange.
        let filename = OsString::from_vec(b"non-utf8-\xff".to_vec());
        context.write_pre_commit_config(indoc::indoc! {r#"
            repos:
              - repo: local
                hooks:
                  - id: mise-filenames
                    name: mise filenames
                    language: mise
                    language_version: "=2026.7.18"
                    entry: sh -c 'for path; do test -e "$path" || exit 1; done' --
                    always_run: true
        "#});
        fs_err::write(context.work_dir().join(&filename), "")?;
        context.git_add(".");

        context.run().arg("mise-filenames").assert().success();
    }

    Ok(())
}

#[test]
fn local_relative_path_dependency_is_rejected() {
    let context = TestContext::new();
    context.init_project();
    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: mise-relative-path
                name: mise relative path
                language: mise
                language_version: system
                entry: tool
                additional_dependencies: ["tool@path:./tool"]
                always_run: true
                pass_filenames: false
    "#});
    context.git_add(".");

    cmd_snapshot!(context.run(), @r"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Failed to install hook `mise-relative-path`
      caused by: local mise hook dependency `tool@path:./tool` must use an absolute `path:` version
    ");
}
