use assert_fs::fixture::{FileWriteStr, PathChild};

use crate::common::{TestContext, cmd_snapshot};

/// Test basic deno hook that runs a simple deno command.
#[test]
fn basic_deno() {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: deno-check
                name: deno check
                language: deno
                entry: deno eval 'console.log("Hello from Deno!")'
                always_run: true
                verbose: true
                pass_filenames: false
    "#});

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    deno check...............................................................Passed
    - hook id: deno-check
    - duration: [TIME]
      Hello from Deno!

    ----- stderr -----
    "#);
}

/// Test `additional_dependencies` are installed correctly.
#[test]
fn additional_dependencies() {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: deno-cowsay
                name: deno cowsay
                language: deno
                entry: deno run -A npm:cowsay Hello World!
                additional_dependencies: ["npm:cowsay"]
                always_run: true
                verbose: true
                pass_filenames: false
    "#});

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r###"
    success: true
    exit_code: 0
    ----- stdout -----
    deno cowsay..............................................................Passed
    - hook id: deno-cowsay
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
    "###);

    // Run again to check `health_check` works correctly.
    cmd_snapshot!(context.filters(), context.run(), @r###"
    success: true
    exit_code: 0
    ----- stdout -----
    deno cowsay..............................................................Passed
    - hook id: deno-cowsay
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
    "###);
}

/// Test built-in deno commands like fmt and lint.
#[test]
fn builtin_commands() -> anyhow::Result<()> {
    let context = TestContext::new();
    context.init_project();

    // Create a deno config file
    context
        .work_dir()
        .child("deno.jsonc")
        .write_str(indoc::indoc! {r#"
        {
          "fmt": {
            "useTabs": false,
            "lineWidth": 80,
            "indentWidth": 2
          }
        }
    "#})?;

    // Create a TypeScript file with formatting issues
    context
        .work_dir()
        .child("test.ts")
        .write_str("const   x   =   1;\nconsole.log( x );\n")?;

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: deno-fmt
                name: Deno format
                entry: deno fmt --config deno.jsonc
                types_or: [file]
                language: deno
              - id: deno-lint
                name: Deno lint
                entry: deno lint --config deno.jsonc
                types_or: [file]
                language: deno
                verbose: true
    "});

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    Deno format..............................................................Failed
    - hook id: deno-fmt
    - files were modified by this hook
      [TEMP_DIR]/test.ts
      Checked 3 files
    Deno lint................................................................Passed
    - hook id: deno-lint
    - duration: [TIME]
      Checked 1 file

    ----- stderr -----
    "#);

    Ok(())
}

/// Test running a Deno script with npm dependencies specified inline.
#[test]
fn deno_script_with_npm_import() -> anyhow::Result<()> {
    let context = TestContext::new();
    context.init_project();

    context
        .work_dir()
        .child("hook.ts")
        .write_str(indoc::indoc! {r#"
        import chalk from "npm:chalk@5";
        console.log(chalk.green("Hello from Deno with chalk!"));
    "#})?;

    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: deno-script
                name: deno script
                language: deno
                entry: deno run -A ./hook.ts
                additional_dependencies: ["npm:chalk@5"]
                always_run: true
                verbose: true
                pass_filenames: false
    "#});

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    deno script..............................................................Passed
    - hook id: deno-script
    - duration: [TIME]
      Hello from Deno with chalk!

    ----- stderr -----
    "#);

    Ok(())
}

/// Test that `DENO_DIR` is properly set for caching.
#[test]
fn deno_dir_isolation() {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: check-deno-dir
                name: check deno dir
                language: deno
                entry: deno eval 'console.log(Deno.env.get("DENO_DIR"))'
                always_run: true
                pass_filenames: false
                verbose: true
    "#});

    context.git_add(".");

    let output = context.run().output().unwrap();
    let stdout_str = String::from_utf8_lossy(&output.stdout);

    // Check that DENO_DIR is set and points to prek's cache directory
    assert!(
        stdout_str.contains("cache/deno"),
        "DENO_DIR should be set to prek's cache directory, got: {stdout_str}"
    );
}

/// Test Deno with multiple npm dependencies.
#[test]
fn multiple_npm_dependencies() {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: deno-multi-deps
                name: deno with multiple deps
                language: deno
                entry: deno eval 'import "npm:chalk@5"; import "npm:cowsay@1"; console.log("Dependencies loaded")'
                additional_dependencies:
                  - npm:chalk@5
                  - npm:cowsay@1
                always_run: true
                verbose: true
                pass_filenames: false
    "#});

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    deno with multiple deps..................................................Passed
    - hook id: deno-multi-deps
    - duration: [TIME]
      Dependencies loaded

    ----- stderr -----
    "#);
}

/// Test that Deno hooks work without any dependencies (like system hooks but with Deno).
#[test]
fn no_dependencies() {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: deno-version
                name: deno version
                language: deno
                entry: deno --version
                always_run: true
                verbose: true
                pass_filenames: false
    "});

    context.git_add(".");

    let filters = context
        .filters()
        .into_iter()
        .chain([(r"deno \d+\.\d+\.\d+.*", "deno X.X.X")])
        .chain([(r"v8 \d+\.\d+\.\d+\.\d+.*", "v8 X.X.X.X")])
        .chain([(r"typescript \d+\.\d+\.\d+", "typescript X.X.X")])
        .collect::<Vec<_>>();

    cmd_snapshot!(filters, context.run(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    deno version.............................................................Passed
    - hook id: deno-version
    - duration: [TIME]
      deno X.X.X
      v8 X.X.X.X
      typescript X.X.X

    ----- stderr -----
    "#);
}
