use assert_fs::fixture::{ChildPath, FileWriteStr, PathChild, PathCreateDir};

use crate::common::{TestEnv, cmd_snapshot};

#[test]
fn local_hook() {
    let context = TestEnv::new()
        .with_file(
            ".Rprofile",
            r#"stop("project .Rprofile should not be loaded")"#,
        )
        .init_git();

    context.write_config(indoc::indoc! {r#"
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

    context.git().add(".");

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
}

#[test]
fn local_hook_with_absolute_additional_dependency() -> anyhow::Result<()> {
    let context = TestEnv::new().init_git();

    write_local_r_package(context.work_dir(), "localdep")?;
    let dependency_path = std::path::absolute(context.child("localdep").path())?;
    let dependency = serde_json::to_string(&dependency_path)?;

    context.write_config(indoc::formatdoc! {r"
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

    context.git().add(".");

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
    let context = TestEnv::new().init_git();
    let hook_repo = context
        .create_hook_repo(
            "r-hook",
            indoc::indoc! {r"
            - id: r-remote
              name: r-remote
              language: r
              entry: Rscript hello.R
        "},
        )
        .with_file("hello.R", "localdep::hello()");
    write_local_r_package(hook_repo.path(), "localdep")?;
    write_renv_project(hook_repo.path())?;

    let hook_repo = hook_repo.build();

    context.write_config(indoc::formatdoc! {r"
        repos:
          - repo: {}
            rev: v1.0.0
            hooks:
              - id: r-remote
                additional_dependencies: [./localdep]
                always_run: true
                verbose: true
                pass_filenames: false
    ", hook_repo});

    context.git().add(".");

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
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
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
    "})
        .init_git();

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
