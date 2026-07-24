use assert_fs::assert::PathAssert;
use assert_fs::fixture::{FileWriteStr, PathChild, PathCreateDir};
use prek_consts::PRE_COMMIT_HOOKS_YAML;
use prek_consts::env_vars::{EnvVars, EnvVarsRead};

use crate::common::{TestContext, cmd_snapshot, make_executable, remove_bin_from_path};

/// Test `language_version` parsing and auto downloading works correctly.
/// We use `setup-node` action to install node 20 in CI, so node 19 should be downloaded by prek.
#[test]
fn language_version() -> anyhow::Result<()> {
    if !EnvVars.is_set(EnvVars::CI) {
        // Skip when not running in CI, as we may have other node versions installed locally.
        return Ok(());
    }

    let context = TestContext::new();
    context.init_project();
    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: node
                name: node
                language: node
                entry: node -p 'process.version'
                language_version: '20'
                always_run: true
              - id: node
                name: node
                language: node
                entry: node -p 'process.version'
                language_version: node20
                always_run: true
              - id: node
                name: node
                language: node
                entry: node -p 'process.version'
                language_version: '19' # will auto download
                always_run: true
              - id: node
                name: node
                language: node
                entry: node -p 'process.version'
                language_version: node19
                always_run: true
              - id: node
                name: node
                language: node
                entry: node -p 'process.version'
                language_version: '<20'
                always_run: true
              - id: node
                name: node
                language: node
                entry: node -p 'process.version'
                language_version: 'lts/iron' # node 20
                always_run: true
    "});
    context.git_add(".");

    let node_dir = context.home_dir().child("tools").child("node");
    node_dir.assert(predicates::path::missing());

    let filters = context
        .filters()
        .into_iter()
        .chain([(r"v(\d+)\.\d+.\d+", "v$1.X.X")])
        .collect::<Vec<_>>();

    cmd_snapshot!(filters, context.run().arg("-v"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    node.....................................................................Passed
    - hook id: node
    - duration: [TIME]

      v20.X.X
    node.....................................................................Passed
    - hook id: node
    - duration: [TIME]

      v20.X.X
    node.....................................................................Passed
    - hook id: node
    - duration: [TIME]

      v19.X.X
    node.....................................................................Passed
    - hook id: node
    - duration: [TIME]

      v19.X.X
    node.....................................................................Passed
    - hook id: node
    - duration: [TIME]

      v19.X.X
    node.....................................................................Passed
    - hook id: node
    - duration: [TIME]

      v20.X.X

    ----- stderr -----
    "#);

    // Check that only node 19 is installed.
    let installed_versions = node_dir
        .read_dir()?
        .flatten()
        .filter_map(|d| {
            let filename = d.file_name().to_string_lossy().into_owned();
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
        "Expected only one node version to be installed, but found: {installed_versions:?}"
    );
    assert!(
        installed_versions.iter().any(|v| v.starts_with("19")),
        "Expected node v19 to be installed, but found: {installed_versions:?}"
    );

    Ok(())
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
              - id: node
                name: node
                language: node
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
    node.....................................................................Passed
    - hook id: node
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

    // Run again to check `health_check` works correctly.
    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    node.....................................................................Passed
    - hook id: node
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

/// Test that remote Node packages are installed through npm's Git package path.
///
/// This runs on every supported npm version. In particular, npm 11.9 through
/// 11.12 must not receive `--allow-git=root` because npm's missing `_isRoot`
/// propagation bug rejects root-level Git dependencies with EALLOWGIT:
/// <https://github.com/npm/cli/issues/9189>
#[test]
fn remote_package_is_installed_from_git() -> anyhow::Result<()> {
    let hook_repo = TestContext::new();
    hook_repo.init_project();

    hook_repo
        .work_dir()
        .child(PRE_COMMIT_HOOKS_YAML)
        .write_str(indoc::indoc! {r"
        - id: remote-node-hook
          name: remote-node-hook
          language: node
          entry: remote-node-hook
          always_run: true
          pass_filenames: false
    "})?;
    hook_repo
        .work_dir()
        .child("package.json")
        .write_str(indoc::indoc! {r#"
        {
          "name": "remote-node-hook",
          "version": "1.0.0",
          "bin": {
            "remote-node-hook": "cli.js"
          }
        }
    "#})?;
    let cli = hook_repo.work_dir().child("cli.js");
    cli.write_str(indoc::indoc! {r#"
        #!/usr/bin/env node
        console.log("remote hook ok");
    "#})?;
    make_executable(cli.path())?;

    hook_repo.git_add(".");
    hook_repo.git_commit("Add remote Node hook");
    hook_repo.git_tag("v1.0.0");

    let context = TestContext::new();
    context.init_project();
    context.write_pre_commit_config(&indoc::formatdoc! {r"
        repos:
          - repo: {}
            rev: v1.0.0
            hooks:
              - id: remote-node-hook
                verbose: true
    ", hook_repo.work_dir().display()});
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    remote-node-hook.........................................................Passed
    - hook id: remote-node-hook
    - duration: [TIME]

      remote hook ok

    ----- stderr -----
    ");

    Ok(())
}

/// A remote Node package's `prepare` script must be able to use its dev dependencies.
///
/// This models packages such as google/gts: the executable is generated by `prepare`,
/// the build tool is a dev dependency, and generated output is not committed. Installing
/// the checkout as a folder runs `prepare` before that dev dependency exists and fails.
/// Installing it as a Git package makes npm prepare a temporary clone after installing
/// its development dependencies.
#[test]
fn remote_prepare_uses_dev_dependencies() -> anyhow::Result<()> {
    let npm = if cfg!(windows) { "npm.cmd" } else { "npm" };
    let Ok(output) = std::process::Command::new(npm).arg("--version").output() else {
        return Ok(());
    };
    let version = semver::Version::parse(String::from_utf8_lossy(&output.stdout).trim())?;
    if version.major < 12 {
        // npm 12 added the non-global nested install that makes this GitFetcher
        // lifecycle ordering work when the outer installation is global.
        return Ok(());
    }

    let hook_repo = TestContext::new();
    hook_repo.init_project();

    hook_repo
        .work_dir()
        .child(PRE_COMMIT_HOOKS_YAML)
        .write_str(indoc::indoc! {r"
        - id: prepared-node-hook
          name: prepared-node-hook
          language: node
          entry: prepared-node-hook
          always_run: true
          pass_filenames: false
    "})?;
    hook_repo
        .work_dir()
        .child("package.json")
        .write_str(indoc::indoc! {r#"
        {
          "name": "prepared-node-hook",
          "version": "1.0.0",
          "bin": {
            "prepared-node-hook": "dist/cli.js"
          },
          "files": [
            "dist"
          ],
          "scripts": {
            "prepare": "tsc"
          },
          "devDependencies": {
            "typescript": "5.6.3"
          }
        }
    "#})?;
    hook_repo
        .work_dir()
        .child("tsconfig.json")
        .write_str(indoc::indoc! {r#"
        {
          "compilerOptions": {
            "module": "CommonJS",
            "outDir": "dist",
            "target": "ES2020"
          },
          "include": [
            "src"
          ]
        }
    "#})?;
    hook_repo
        .work_dir()
        .child(".gitignore")
        .write_str("dist/\nnode_modules/\n")?;

    let source = hook_repo.work_dir().child("src");
    source.create_dir_all()?;
    source.child("cli.ts").write_str(indoc::indoc! {r#"
        #!/usr/bin/env node
        console.log("prepared hook ok");
    "#})?;

    hook_repo.git_add(".");
    hook_repo.git_commit("Add source-built Node hook");
    hook_repo.git_tag("v1.0.0");

    let context = TestContext::new();
    context.init_project();
    context.write_pre_commit_config(&indoc::formatdoc! {r"
        repos:
          - repo: {}
            rev: v1.0.0
            hooks:
              - id: prepared-node-hook
                verbose: true
    ", hook_repo.work_dir().display()});
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    prepared-node-hook.......................................................Passed
    - hook id: prepared-node-hook
    - duration: [TIME]

      prepared hook ok

    ----- stderr -----
    ");

    Ok(())
}

/// Test that lowercase npm config inherited from `npm exec` cannot redirect installs.
#[test]
fn additional_dependencies_ignore_inherited_npm_config_prefix() -> anyhow::Result<()> {
    let context = TestContext::new();
    context.init_project();

    let package_dir = context.work_dir().child("prefix-fixture");
    package_dir.create_dir_all()?;
    package_dir
        .child("package.json")
        .write_str(indoc::indoc! {r#"
        {
          "name": "prek-prefix-fixture",
          "version": "1.0.0",
          "bin": {
            "prek-prefix-fixture": "cli.js"
          }
        }
    "#})?;
    let cli = package_dir.child("cli.js");
    cli.write_str(indoc::indoc! {r#"
        #!/usr/bin/env node
        console.log("prefix fixture ok")
    "#})?;
    make_executable(cli.path())?;

    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: node
                name: node
                language: node
                entry: prek-prefix-fixture
                additional_dependencies: ["./prefix-fixture"]
                always_run: true
                verbose: true
                pass_filenames: false
    "#});

    context.git_add(".");

    let fake_prefix = context.home_dir().child("fake-prefix");
    fake_prefix.create_dir_all()?;
    let global_npmrc = fake_prefix.child("global-npmrc");
    let user_npmrc = fake_prefix.child("user-npmrc");
    global_npmrc.write_str("prefix=${HOME}/global-npmrc-prefix\n")?;
    user_npmrc.write_str("//registry.example.test/:_authToken=fake-token\n")?;

    cmd_snapshot!(
        context.filters(),
        context
            .run()
            .env("npm_config_prefix", fake_prefix.path())
            .env("npm_config_global_prefix", fake_prefix.path())
            .env("npm_config_local_prefix", fake_prefix.path())
            .env("npm_config_globalconfig", global_npmrc.path())
            .env("npm_config_userconfig", user_npmrc.path())
            .env("npm_config_cache", fake_prefix.child("cache").path()),
        @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    node.....................................................................Passed
    - hook id: node
    - duration: [TIME]

      prefix fixture ok

    ----- stderr -----
    "#
    );

    fake_prefix
        .child("lib")
        .child("node_modules")
        .assert(predicates::path::missing());

    Ok(())
}

/// Test that npm install works without system node in PATH.
/// Regression test for #1492: `install()` must use the provisioned toolchain.
#[test]
fn additional_dependencies_without_system_node() -> anyhow::Result<()> {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: node
                name: node
                language: node
                entry: cowsay Hello
                additional_dependencies: ["cowsay"]
                always_run: true
                pass_filenames: false
    "#});

    context.git_add(".");

    let new_path = remove_bin_from_path("node", None)?;

    cmd_snapshot!(context.filters(), context.run().env("PATH", new_path), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    node.....................................................................Passed

    ----- stderr -----
    ");

    Ok(())
}

/// Test that `npm.cmd` can be found on Windows.
#[test]
fn npm_version() {
    let context = TestContext::new();
    context.init_project();
    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: npm-version
                name: npm-version
                language: system
                entry: npm --version
                always_run: true
                pass_filenames: false
                verbose: true
    "});
    context.git_add(".");

    let filters = context
        .filters()
        .into_iter()
        .chain([(r"\d+\.\d+\.\d+", "[NPM_VERSION]")])
        .collect::<Vec<_>>();

    cmd_snapshot!(filters, context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    npm-version..............................................................Passed
    - hook id: npm-version
    - duration: [TIME]

      [NPM_VERSION]

    ----- stderr -----
    ");
}
