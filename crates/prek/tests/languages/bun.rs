use anyhow::Result;
use assert_fs::assert::PathAssert;
use assert_fs::fixture::PathChild;
use prek_consts::env_vars::EnvVars;

use crate::common::{TestContext, cmd_snapshot};

/// Test basic Bun hook execution.
#[test]
fn basic_bun() {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: bun-check
                name: bun check
                language: bun
                entry: bun -e 'console.log("Hello from Bun!")'
                always_run: true
                verbose: true
                pass_filenames: false
    "#});

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    bun check................................................................Passed
    - hook id: bun-check
    - duration: [TIME]

      Hello from Bun!

    ----- stderr -----
    ");
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
              - id: bun-cowsay
                name: bun cowsay
                language: bun
                entry: cowsay Hello World!
                additional_dependencies: ["cowsay"]
                always_run: true
                verbose: true
                pass_filenames: false
    "#});

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    bun cowsay...............................................................Passed
    - hook id: bun-cowsay
    - duration: [TIME]

      ______________
      < Hello World! >
       --------------
              \   ^__^
               \  (oo)/_______
                  (__)\       )\/\
                      ||----w |
                      ||     ||

    ----- stderr -----
    ");

    // Run again to check `health_check` works correctly (cache reuse).
    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    bun cowsay...............................................................Passed
    - hook id: bun-cowsay
    - duration: [TIME]

      ______________
      < Hello World! >
       --------------
              \   ^__^
               \  (oo)/_______
                  (__)\       )\/\
                      ||----w |
                      ||     ||

    ----- stderr -----
    ");
}

/// Test `language_version` specification and bun installation.
/// In CI, we ensure bun 1.3 is installed.
#[test]
fn language_version() -> Result<()> {
    if !EnvVars::is_set(EnvVars::CI) {
        // Skip when not running in CI, as we may have other go versions installed locally.
        return Ok(());
    }

    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: bun-version
                name: bun version check
                language: bun
                language_version: ">1.2"
                entry: bun -e 'console.log(`Bun ${Bun.version}`)'
                always_run: true
                verbose: true
                pass_filenames: false
              - id: bun-version
                name: bun version check
                language: bun
                language_version: "1.3"
                entry: bun -e 'console.log(`Bun ${Bun.version}`)'
                always_run: true
                verbose: true
                pass_filenames: false
              - id: bun-version
                name: bun version check
                language: bun
                language_version: "1.2" # will auto download
                entry: bun -e 'console.log(`Bun ${Bun.version}`)'
                always_run: true
                verbose: true
                pass_filenames: false
              - id: bun-version
                name: bun version check
                language: bun
                language_version: "bun@1.2"
                entry: bun -e 'console.log(`Bun ${Bun.version}`)'
                always_run: true
                verbose: true
                pass_filenames: false
                additional_dependencies: ["cowsay"] # different dep to force create separate env
    "#});

    context.git_add(".");

    let bun_dir = context.home_dir().child("tools").child("bun");
    bun_dir.assert(predicates::path::missing());

    let filters = context
        .filters()
        .into_iter()
        .chain([(r"Bun (\d+\.\d+)\.\d+", "Bun $1.X")])
        .collect::<Vec<_>>();

    cmd_snapshot!(filters, context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    bun version check........................................................Passed
    - hook id: bun-version
    - duration: [TIME]

      Bun 1.3.X
    bun version check........................................................Passed
    - hook id: bun-version
    - duration: [TIME]

      Bun 1.3.X
    bun version check........................................................Passed
    - hook id: bun-version
    - duration: [TIME]

      Bun 1.2.X
    bun version check........................................................Passed
    - hook id: bun-version
    - duration: [TIME]

      Bun 1.2.X

    ----- stderr -----
    ");

    // Check that only bun 1.2 is installed.
    let installed_versions = bun_dir
        .read_dir()?
        .flatten()
        .filter_map(|d| {
            let filename = d.file_name().to_string_lossy().to_string();
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
        "Expected only one Bun version to be installed, but found: {installed_versions:?}"
    );
    assert!(
        installed_versions.iter().any(|v| v.contains("1.2")),
        "Expected Bun 1.2 to be installed, but found: {installed_versions:?}"
    );

    Ok(())
}
