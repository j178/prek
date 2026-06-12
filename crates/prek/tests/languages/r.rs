use assert_fs::fixture::{FileWriteStr, PathChild, PathCreateDir};
use prek_consts::PRE_COMMIT_HOOKS_YAML;

use crate::common::{TestContext, cmd_snapshot};

#[test]
fn local_hook() {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: r-local
                name: r-local
                language: r
                entry: Rscript -e 'cat("Hello from R!\n")'
                always_run: true
                verbose: true
                pass_filenames: false
    "#});

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    r-local.................................................................Passed
    - hook id: r-local
    - duration: [TIME]

      Hello from R!

    ----- stderr -----
    ");
}

#[test]
fn local_hook_with_relative_additional_dependency() -> anyhow::Result<()> {
    let context = TestContext::new();
    context.init_project();

    write_local_r_package(&context, "localdep")?;

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: r-local-dep
                name: r-local-dep
                language: r
                entry: Rscript -e 'localdep::hello()'
                additional_dependencies: [localdep]
                always_run: true
                verbose: true
                pass_filenames: false
    "});

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    r-local-dep.............................................................Passed
    - hook id: r-local-dep
    - duration: [TIME]

      Hello from local R dependency!

    ----- stderr -----
    ");

    Ok(())
}

#[test]
fn remote_repo_install() -> anyhow::Result<()> {
    let hook_repo = TestContext::new();
    hook_repo.init_project();

    hook_repo
        .work_dir()
        .child(PRE_COMMIT_HOOKS_YAML)
        .write_str(indoc::indoc! {r"
            - id: r-remote
              name: r-remote
              language: r
              entry: Rscript hello.R
        "})?;
    hook_repo
        .work_dir()
        .child("hello.R")
        .write_str(r#"cat("Hello from remote R!\n")"#)?;
    write_renv_project(&hook_repo)?;

    hook_repo.git_add(".");
    hook_repo.git_commit("Add R hook");
    hook_repo.git_tag("v1.0.0");

    let context = TestContext::new();
    context.init_project();
    context.write_pre_commit_config(&indoc::formatdoc! {r"
        repos:
          - repo: {}
            rev: v1.0.0
            hooks:
              - id: r-remote
                always_run: true
                verbose: true
                pass_filenames: false
    ", hook_repo.work_dir().display()});

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    r-remote................................................................Passed
    - hook id: r-remote
    - duration: [TIME]

      Hello from remote R!

    ----- stderr -----
    ");

    Ok(())
}

#[test]
fn language_version() {
    let context = TestContext::new();
    context.init_project();
    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: r-version
                name: r-version
                language: r
                entry: Rscript -e 'cat(getRversion())'
                language_version: '4.4'
                always_run: true
                verbose: true
                pass_filenames: false
    "});

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Failed to init hooks
      caused by: Invalid hook `r-version`
      caused by: Hook specified `language_version: 4.4` but the language `r` does not support toolchain installation for now
    ");
}

fn write_local_r_package(context: &TestContext, name: &str) -> anyhow::Result<()> {
    let package_dir = context.work_dir().child(name);
    package_dir.create_dir_all()?;
    package_dir
        .child("DESCRIPTION")
        .write_str(&indoc::formatdoc! {r"
            Package: {name}
            Version: 0.1.0
            Title: Local Test Package
            Description: Local test package for R hook integration tests.
            License: MIT
            Encoding: UTF-8
        "})?;
    package_dir
        .child("NAMESPACE")
        .write_str("export(hello)\n")?;
    package_dir.child("R").create_dir_all()?;
    package_dir
        .child("R")
        .child("hello.R")
        .write_str(indoc::indoc! {r#"
            hello <- function() {
              cat("Hello from local R dependency!\n")
            }
        "#})?;
    Ok(())
}

fn write_renv_project(context: &TestContext) -> anyhow::Result<()> {
    context
        .work_dir()
        .child("renv.lock")
        .write_str(indoc::indoc! {r#"
            {
              "R": {
                "Version": "4.4.0",
                "Repositories": [
                  {
                    "Name": "CRAN",
                    "URL": "https://cloud.r-project.org"
                  }
                ]
              },
              "Packages": {}
            }
        "#})?;
    let renv_dir = context.work_dir().child("renv");
    renv_dir.create_dir_all()?;
    renv_dir.child("activate.R").write_str(indoc::indoc! {r#"
            lib_dir <- file.path(getwd(), "library")
            dir.create(lib_dir, recursive = TRUE, showWarnings = FALSE)
            .libPaths(c(lib_dir, .libPaths()))
            if (!requireNamespace("renv", quietly = TRUE)) {
              install.packages("renv", lib = lib_dir, repos = c(CRAN = "https://cran.rstudio.com"))
            }
        "#})?;
    Ok(())
}
