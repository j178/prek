use assert_fs::fixture::{ChildPath, FileWriteStr, PathChild, PathCreateDir};
use prek_consts::PRE_COMMIT_HOOKS_YAML;

use crate::common::{TestEnv, cmd_snapshot};

#[test]
fn local_hook() -> anyhow::Result<()> {
    let context = TestEnv::new();
    context
        .work_dir()
        .child(".Rprofile")
        .write_str(r#"stop("project .Rprofile should not be loaded")"#)?;

    let context = context.with_config(indoc::indoc! {r#"
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

    context.git_add_all();

    cmd_snapshot!(context, context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    r-local..................................................................Passed
    - hook id: r-local
    - duration: [TIME]

      Hello from R!

    ----- stderr -----
    ");

    // Run again to verify the `check_health` logic.
    cmd_snapshot!(context, context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    r-local..................................................................Passed
    - hook id: r-local
    - duration: [TIME]

      Hello from R!

    ----- stderr -----
    ");

    Ok(())
}

#[test]
fn local_hook_with_absolute_additional_dependency() -> anyhow::Result<()> {
    let context = TestEnv::new();

    write_local_r_package(context.work_dir(), "localdep")?;
    let dependency_path = std::path::absolute(context.work_dir().child("localdep").path())?;
    let dependency = serde_json::to_string(&dependency_path)?;

    let context = context.with_config(indoc::formatdoc! {r"
        repos:
          - repo: local
            hooks:
              - id: r-local-dep
                name: r-local-dep
                language: r
                entry: Rscript -e 'localdep::hello()'
                additional_dependencies: [{dependency}]
                always_run: true
                verbose: true
                pass_filenames: false
    "});

    context.git_add_all();

    cmd_snapshot!(context, context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    r-local-dep..............................................................Passed
    - hook id: r-local-dep
    - duration: [TIME]

      Hello from local R dependency!

    ----- stderr -----
    ");

    Ok(())
}

#[test]
fn remote_repo_install() -> anyhow::Result<()> {
    let context = TestEnv::new();
    let hook_repo = context.create_repo("r-hook");

    hook_repo
        .path()
        .child(PRE_COMMIT_HOOKS_YAML)
        .write_str(indoc::indoc! {r"
            - id: r-remote
              name: r-remote
              language: r
              entry: Rscript hello.R
        "})?;
    hook_repo
        .path()
        .child("hello.R")
        .write_str("localdep::hello()")?;
    write_local_r_package(hook_repo.path(), "localdep")?;
    write_renv_project(hook_repo.path())?;

    hook_repo.git_add_all();
    hook_repo.git_commit("Add R hook");
    hook_repo.git_tag("v1.0.0");

    let context = context.with_config(indoc::formatdoc! {r"
        repos:
          - repo: {}
            rev: v1.0.0
            hooks:
              - id: r-remote
                additional_dependencies: [./localdep]
                always_run: true
                verbose: true
                pass_filenames: false
    ", hook_repo.path().display()});

    context.git_add_all();

    cmd_snapshot!(context, context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    r-remote.................................................................Passed
    - hook id: r-remote
    - duration: [TIME]

      Hello from local R dependency!

    ----- stderr -----
    ");

    Ok(())
}

#[test]
fn language_version() {
    let context = TestEnv::new().with_config(indoc::indoc! {r"
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

    context.git_add_all();

    cmd_snapshot!(context, context.run(), @r"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Failed to init hooks
      caused by: Invalid hook `r-version`
      caused by: Hook specified `language_version: 4.4` but the language `r` does not support toolchain installation for now
    ");
}

fn write_local_r_package(work_dir: &ChildPath, name: &str) -> anyhow::Result<()> {
    let package_dir = work_dir.child(name);
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

fn write_renv_project(work_dir: &ChildPath) -> anyhow::Result<()> {
    work_dir.child("renv.lock").write_str(indoc::indoc! {r#"
            {
              "R": {
                "Version": "4.6.0",
                "Repositories": [
                  {
                    "Name": "CRAN",
                    "URL": "https://cran.rstudio.com"
                  }
                ]
              },
              "Packages": {
                "renv": {
                  "Package": "renv",
                  "Version": "1.2.3",
                  "Source": "Repository",
                  "Repository": "CRAN"
                }
              }
            }
        "#})?;
    let renv_dir = work_dir.child("renv");
    renv_dir.create_dir_all()?;
    renv_dir.child("activate.R").write_str(indoc::indoc! {r#"
            lib_dir <- file.path(getwd(), "library")
            dir.create(lib_dir, recursive = TRUE, showWarnings = FALSE)
            .libPaths(c(lib_dir, .libPaths()))
            if (!requireNamespace("renv", quietly = TRUE)) {
              install.packages(
                "renv",
                lib = lib_dir,
                repos = c(CRAN = "https://cran.rstudio.com"),
                type = .Platform$pkgType
              )
            }
            renv::load(getwd(), quiet = TRUE)
        "#})?;

    Ok(())
}
