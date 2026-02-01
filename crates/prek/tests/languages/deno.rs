use assert_fs::assert::PathAssert;
use assert_fs::fixture::{FileWriteStr, PathChild};
use prek_consts::env_vars::EnvVars;

use crate::common::{TestContext, cmd_snapshot, remove_bin_from_path};

/// Test basic Deno hook execution with an inline script.
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

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    deno check...............................................................Passed
    - hook id: deno-check
    - duration: [TIME]

      Hello from Deno!

    ----- stderr -----
    ");
}

/// Test running a TypeScript script file.
/// This validates the `build_deno_command` fix that prepends `deno run` for script entries.
#[test]
fn script_file() {
    let context = TestContext::new();
    context.init_project();

    // Create a TypeScript script
    context
        .work_dir()
        .child("check.ts")
        .write_str(indoc::indoc! {r#"
            console.log("Script executed successfully!");
        "#})
        .expect("Failed to write check.ts");

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: ts-script
                name: ts script
                language: deno
                entry: ./check.ts
                always_run: true
                verbose: true
                pass_filenames: false
    "});

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    ts script................................................................Passed
    - hook id: ts-script
    - duration: [TIME]

      Script executed successfully!

    ----- stderr -----
    ");
}

/// Test running Deno built-in subcommands (fmt, lint, etc.).
/// These should be passed directly to deno without prepending `deno run`.
#[test]
fn builtin_commands() {
    let context = TestContext::new();
    context.init_project();

    // Create a TypeScript file for formatting check
    context
        .work_dir()
        .child("example.ts")
        .write_str(indoc::indoc! {r"
        const x = 1;
        console.log(x);
    "})
        .expect("Failed to write example.ts");

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: deno-fmt-check
                name: deno fmt check
                language: deno
                entry: fmt --check
                types: [ts]
                verbose: true
    "});

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    deno fmt check...........................................................Passed
    - hook id: deno-fmt-check
    - duration: [TIME]

      Checked 1 file

    ----- stderr -----
    ");
}

/// Test that explicit `deno` prefix is handled correctly.
#[test]
fn explicit_deno_prefix() {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: deno-version
                name: deno version
                language: deno
                entry: deno eval 'console.log(Deno.version.deno)'
                always_run: true
                verbose: true
                pass_filenames: false
    "});

    context.git_add(".");

    let filters = context
        .filters()
        .into_iter()
        .chain([(r"\d+\.\d+\.\d+", "[DENO_VERSION]")])
        .collect::<Vec<_>>();

    cmd_snapshot!(filters, context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    deno version.............................................................Passed
    - hook id: deno-version
    - duration: [TIME]

      [DENO_VERSION]

    ----- stderr -----
    ");
}

/// Test that `additional_dependencies` are installed correctly.
/// Deno caches npm packages when they're imported.
#[test]
fn additional_dependencies() {
    let context = TestContext::new();
    context.init_project();

    // Create a script that uses an npm dependency (lodash is simpler than chalk)
    context
        .work_dir()
        .child("use_lodash.ts")
        .write_str(indoc::indoc! {r#"
        import _ from "npm:lodash@4";
        console.log(_.capitalize("hello from lodash"));
    "#})
        .expect("Failed to write use_lodash.ts");

    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: deno-lodash
                name: deno lodash
                language: deno
                entry: ./use_lodash.ts
                additional_dependencies: ["npm:lodash@4"]
                always_run: true
                verbose: true
                pass_filenames: false
    "#});

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    deno lodash..............................................................Passed
    - hook id: deno-lodash
    - duration: [TIME]

      Hello from lodash

    ----- stderr -----
    ");

    // Run again to check `health_check` works correctly (cache reuse).
    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    deno lodash..............................................................Passed
    - hook id: deno-lodash
    - duration: [TIME]

      Hello from lodash

    ----- stderr -----
    ");
}

/// Test `language_version` specification and deno installation.
/// In CI, we ensure deno 2.x is installed via setup-deno action.
#[test]
fn language_version() {
    if !EnvVars::is_set(EnvVars::CI) {
        // Skip when not running in CI, as we may have other deno versions installed locally.
        return;
    }

    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: deno-version
                name: deno version check (system)
                language: deno
                language_version: '2'
                entry: deno eval 'console.log(`Deno ${Deno.version.deno}`)'
                always_run: true
                verbose: true
                pass_filenames: false
              - id: deno-version
                name: deno version check (deno@2)
                language: deno
                language_version: deno@2
                entry: deno eval 'console.log(`Deno ${Deno.version.deno}`)'
                always_run: true
                verbose: true
                pass_filenames: false
              - id: deno-version
                name: deno version check (1.46 - will auto download)
                language: deno
                language_version: '1.46'
                entry: deno eval 'console.log(`Deno ${Deno.version.deno}`)'
                always_run: true
                verbose: true
                pass_filenames: false
              - id: deno-version
                name: deno version check (deno@1.46)
                language: deno
                language_version: deno@1.46
                entry: deno eval 'console.log(`Deno ${Deno.version.deno}`)'
                always_run: true
                verbose: true
                pass_filenames: false
    "});

    context.git_add(".");

    let deno_dir = context.home_dir().child("tools").child("deno");
    deno_dir.assert(predicates::path::missing());

    // Use two filters: first masks minor+patch for Deno 2.x (major-only request),
    // then masks only patch for specific minor versions like 1.46.x
    let filters = context
        .filters()
        .into_iter()
        .chain([
            (r"Deno 2\.\d+\.\d+", "Deno 2.X.X"),
            (r"Deno (\d+\.\d+)\.\d+", "Deno $1.X"),
        ])
        .collect::<Vec<_>>();

    cmd_snapshot!(filters, context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    deno version check (system)..............................................Passed
    - hook id: deno-version
    - duration: [TIME]

      Deno 2.X.X
    deno version check (deno@2)..............................................Passed
    - hook id: deno-version
    - duration: [TIME]

      Deno 2.X.X
    deno version check (1.46 - will auto download)...........................Passed
    - hook id: deno-version
    - duration: [TIME]

      Deno 1.46.X
    deno version check (deno@1.46)...........................................Passed
    - hook id: deno-version
    - duration: [TIME]

      Deno 1.46.X

    ----- stderr -----
    ");

    // Check that only deno 1.46 is installed (2.x uses system).
    let installed_versions = deno_dir
        .read_dir()
        .expect("Failed to read deno tools directory")
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
        "Expected only one Deno version to be installed, but found: {installed_versions:?}"
    );
    assert!(
        installed_versions.iter().any(|v| v.contains("1.46")),
        "Expected Deno 1.46 to be installed, but found: {installed_versions:?}"
    );
}

/// Test that deno hooks work without system deno in PATH.
/// Regression test ensuring `install()` uses the provisioned toolchain.
#[test]
fn without_system_deno() {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: deno-check
                name: deno check
                language: deno
                entry: deno eval 'console.log("Hello")'
                always_run: true
                pass_filenames: false
    "#});

    context.git_add(".");

    let new_path = remove_bin_from_path("deno", None).expect("Failed to remove deno from PATH");

    cmd_snapshot!(context.filters(), context.run().env("PATH", new_path), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    deno check...............................................................Passed

    ----- stderr -----
    ");
}

/// Test semver range version specification.
#[test]
fn version_range() {
    if !EnvVars::is_set(EnvVars::CI) {
        // Skip when not running in CI.
        return;
    }

    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: deno-version
                name: deno version range
                language: deno
                language_version: ">=2.0"
                entry: deno eval 'console.log(`Deno ${Deno.version.deno}`)'
                always_run: true
                verbose: true
                pass_filenames: false
    "#});

    context.git_add(".");

    let filters = context
        .filters()
        .into_iter()
        .chain([(r"Deno \d+\.\d+\.\d+", "Deno [VERSION]")])
        .collect::<Vec<_>>();

    cmd_snapshot!(filters, context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    deno version range.......................................................Passed
    - hook id: deno-version
    - duration: [TIME]

      Deno [VERSION]

    ----- stderr -----
    ");
}

/// Test that hook failure is properly reported.
#[test]
fn hook_failure() {
    let context = TestContext::new();
    context.init_project();

    // Create a TypeScript file with a lint error
    context
        .work_dir()
        .child("bad.ts")
        .write_str(indoc::indoc! {r"
        // This has a lint error: no-explicit-any
        let x: any = 1;
        console.log(x);
    "})
        .expect("Failed to write bad.ts");

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: deno-lint
                name: deno lint
                language: deno
                entry: lint
                types: [ts]
                verbose: true
    "});

    context.git_add(".");

    // The lint should fail due to no-explicit-any
    let output = context.run().output().expect("Failed to run hook");
    assert!(!output.status.success(), "Expected lint to fail");
}

/// Test script with Deno permissions.
/// Note: Permissions must come before the script in the entry, so use explicit `deno run`.
#[test]
fn script_with_permissions() {
    let context = TestContext::new();
    context.init_project();

    // Create a script that reads an environment variable
    context
        .work_dir()
        .child("read_env.ts")
        .write_str(indoc::indoc! {r#"
        console.log(Deno.env.get("TEST_VAR") ?? "not set");
    "#})
        .expect("Failed to write read_env.ts");

    // Permissions must be specified before the script path when using deno run
    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: deno-env
                name: deno env
                language: deno
                entry: deno run --allow-env ./read_env.ts
                always_run: true
                verbose: true
                pass_filenames: false
    "});

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run().env("TEST_VAR", "hello"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    deno env.................................................................Passed
    - hook id: deno-env
    - duration: [TIME]

      hello

    ----- stderr -----
    ");
}
