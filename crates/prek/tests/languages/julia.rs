use crate::common::{TestContext, cmd_snapshot};

#[test]
fn local_hook() {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: julia-test
                name: julia-test
                language: julia
                entry: -e 'println("Hello from Julia!")'
                always_run: true
                verbose: true
                pass_filenames: false
    "#});

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    julia-test...............................................................Passed
    - hook id: julia-test
    - duration: [TIME]

      Hello from Julia!

    ----- stderr -----
    ");

    // Run again to check `health_check` works correctly.
    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    julia-test...............................................................Passed
    - hook id: julia-test
    - duration: [TIME]

      Hello from Julia!

    ----- stderr -----
    ");
}

#[test]
fn additional_dependencies() {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: julia-deps
                name: julia-deps
                language: julia
                entry: -e 'using JSON; println("JSON module loaded")'
                additional_dependencies: ["JSON"]
                always_run: true
                verbose: true
                pass_filenames: false
    "#});

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    julia-deps...............................................................Passed
    - hook id: julia-deps
    - duration: [TIME]

      JSON module loaded

    ----- stderr -----
    ");
}

#[test]
fn project_toml() -> anyhow::Result<()> {
    use assert_fs::fixture::{FileWriteStr, PathChild};

    let context = TestContext::new();
    context.init_project();

    context
        .work_dir()
        .child("Project.toml")
        .write_str(indoc::indoc! {r#"
            [deps]
            Example = "7876af07-990d-54b4-ab0e-23690620f79a"
        "#})?;

    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: julia-project
                name: julia-project
                language: julia
                entry: -e 'using Example; println("Example module loaded")'
                always_run: true
                verbose: true
                pass_filenames: false
    "#});

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    julia-project............................................................Passed
    - hook id: julia-project
    - duration: [TIME]

      Example module loaded

    ----- stderr -----
    ");

    Ok(())
}

#[test]
fn script_file() -> anyhow::Result<()> {
    use assert_fs::fixture::{FileWriteStr, PathChild};

    let context = TestContext::new();
    context.init_project();

    context
        .work_dir()
        .child("my_script.jl")
        .write_str(r#"println("Hello from script file!")"#)?;

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: julia-script
                name: julia-script
                language: julia
                entry: my_script.jl
                always_run: true
                verbose: true
                pass_filenames: false
    "});

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    julia-script.............................................................Passed
    - hook id: julia-script
    - duration: [TIME]

      Hello from script file!

    ----- stderr -----
    ");

    Ok(())
}

#[test]
fn remote_hook() {
    let context = TestContext::new();

    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: https://github.com/prek-test-repos/julia-hooks
            rev: v1.0.0
            hooks:
              - id: hello
                always_run: true
                verbose: true
    "});

    context.git_add(".");

    let filters = context.filters();

    cmd_snapshot!(filters, context.run(), @"
    success: true
    exit_code: 0
    ----- stdout -----
    hello....................................................................Passed
    - hook id: hello
    - duration: [TIME]

      This is a remote julia hook
      Args: hello

    ----- stderr -----
    ");
}
