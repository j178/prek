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
                entry: bunx cowsay Hello World!
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

/// Test `language_version` specification works correctly.
#[test]
fn language_version() {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: bun-version
                name: bun version check
                language: bun
                language_version: "1"
                entry: bun -e 'console.log(`Bun ${Bun.version}`)'
                always_run: true
                verbose: true
                pass_filenames: false
    "#});

    context.git_add(".");

    let filters = context
        .filters()
        .into_iter()
        .chain([(r"Bun \d+\.\d+\.\d+", "Bun [VERSION]")])
        .collect::<Vec<_>>();

    cmd_snapshot!(filters, context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    bun version check........................................................Passed
    - hook id: bun-version
    - duration: [TIME]

      Bun [VERSION]

    ----- stderr -----
    ");
}
