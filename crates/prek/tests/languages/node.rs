use assert_cmd::assert::OutputAssertExt;
use assert_fs::assert::PathAssert;
use assert_fs::fixture::{FileWriteStr, PathChild, PathCreateDir};
use prek_consts::env_vars::EnvVars;
use url::Url;

use crate::common::{TestEnv, cmd_snapshot, remove_bin_from_path};

#[test]
fn exec_uses_installed_node_environment() -> anyhow::Result<()> {
    let context = TestEnv::new()
        .with_file(
            "node-env-tool/package.json",
            indoc::indoc! {r#"
        {
          "name": "node-env-tool",
          "version": "1.0.0",
          "bin": {
            "node-env-tool": "cli.js"
          }
        }
    "#},
        )
        .with_executable_file(
            "node-env-tool/cli.js",
            indoc::indoc! {r#"
        #!/usr/bin/env node
        console.log("exec node env ok");
    "#},
        )
        .init_git();
    let package = context.child("node-env-tool");

    let dependency = serde_json::to_string(package.path())?;
    context.write_config(indoc::formatdoc! {r"
        repos:
          - repo: local
            hooks:
              - id: node
                name: node
                language: node
                entry: command-that-must-not-run
                additional_dependencies: [{dependency}]
    "});

    cmd_snapshot!(context, context.exec().args([
        "node",
        "--",
        "node-env-tool",
    ]), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    exec node env ok

    ----- stderr -----
    ");

    Ok(())
}

/// Test `language_version` parsing and auto downloading works correctly.
/// We use `setup-node` action to install node 20 in CI, so node 19 should be downloaded by prek.
#[cfg(feature = "ci")]
#[test]
fn language_version() -> anyhow::Result<()> {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
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
    "})
        .init_git();

    let node_dir = context.home_dir().child("tools").child("node");
    node_dir.assert(predicates::path::missing());

    let context = context.with_filter(r"v(\d+)\.\d+.\d+", "v$1.X.X");

    cmd_snapshot!(context, context.run().arg("-v"), @r#"
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
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r#"
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
    "#})
        .init_git();

    cmd_snapshot!(context, context.run(), @r"
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
    cmd_snapshot!(context, context.run(), @r"
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

/// Test that remote Node packages with runtime dependencies are prepared through npm's Git
/// package path before installation.
///
/// This runs on every supported npm version. In particular, npm 11.9 through
/// 11.12 must not receive `--allow-git=root` because npm's missing `_isRoot`
/// propagation bug rejects root-level Git dependencies with EALLOWGIT:
/// <https://github.com/npm/cli/issues/9189>
#[test]
fn remote_package_is_installed_from_git() {
    let context = TestEnv::new().init_git();
    let hook_repo = context
        .create_hook_repo(
            "remote-node-hook",
            indoc::indoc! {r"
                - id: remote-node-hook
                  name: remote-node-hook
                  language: node
                  entry: remote-node-hook
                  always_run: true
                  pass_filenames: false
            "},
        )
        .with_file(
            "package.json",
            indoc::indoc! {r#"
                {
                  "name": "remote-node-hook",
                  "version": "1.0.0",
                  "bin": {
                    "remote-node-hook": "cli.js"
                  },
                  "dependencies": {
                    "is-number": "7.0.0"
                  }
                }
            "#},
        )
        .with_executable_file(
            "cli.js",
            indoc::indoc! {r#"
        #!/usr/bin/env node
        const isNumber = require("is-number");
        if (!isNumber(42)) process.exit(1);
        console.log("remote hook ok");
    "#},
        )
        .build();

    context.write_config(indoc::formatdoc! {r"
        repos:
          - repo: {}
            rev: v1.0.0
            hooks:
              - id: remote-node-hook
                verbose: true
    ", hook_repo});
    context.git().add(".");

    cmd_snapshot!(context, context.run().env(EnvVars::PREK_HOME, ".prek-cache"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    remote-node-hook.........................................................Passed
    - hook id: remote-node-hook
    - duration: [TIME]

      remote hook ok

    ----- stderr -----
    ");
}

/// A remote Node package's `prepare` script must be able to use its dev dependencies.
///
/// This models packages such as google/gts: the executable is generated by `prepare`,
/// the build tool is a dev dependency, and generated output is not committed. Installing
/// the checkout as a folder runs `prepare` before that dev dependency exists and fails.
/// Installing it as a Git package makes npm prepare a temporary clone after installing
/// its development dependencies.
#[test]
fn remote_prepare_uses_dev_dependencies() {
    let context = TestEnv::new().init_git();
    let hook_repo = context
        .create_hook_repo(
            "prepared-node-hook",
            indoc::indoc! {r"
                - id: prepared-node-hook
                  name: prepared-node-hook
                  language: node
                  entry: prepared-node-hook
                  always_run: true
                  pass_filenames: false
            "},
        )
        .with_file(
            "package.json",
            indoc::indoc! {r#"
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
            "#},
        )
        .with_file(
            "tsconfig.json",
            indoc::indoc! {r#"
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
            "#},
        )
        .with_file(".gitignore", "dist/\nnode_modules/\n")
        .with_file(
            "src/cli.ts",
            indoc::indoc! {r#"
                #!/usr/bin/env node
                console.log("prepared hook ok");
        "#},
        )
        .build();

    context.write_config(indoc::formatdoc! {r"
        repos:
          - repo: {}
            rev: v1.0.0
            hooks:
              - id: prepared-node-hook
                verbose: true
    ", hook_repo});
    context.git().add(".");

    cmd_snapshot!(context, context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    prepared-node-hook.......................................................Passed
    - hook id: prepared-node-hook
    - duration: [TIME]

      prepared hook ok

    ----- stderr -----
    ");
}

/// Test that lowercase npm config inherited from `npm exec` cannot redirect installs.
#[test]
fn additional_dependencies_ignore_inherited_npm_config_prefix() -> anyhow::Result<()> {
    let context = TestEnv::new()
        .with_file(
            "prefix-fixture/package.json",
            indoc::indoc! {r#"
        {
          "name": "prek-prefix-fixture",
          "version": "1.0.0",
          "bin": {
            "prek-prefix-fixture": "cli.js"
          }
        }
    "#},
        )
        .with_executable_file(
            "prefix-fixture/cli.js",
            indoc::indoc! {r#"
        #!/usr/bin/env node
        console.log("prefix fixture ok")
    "#},
        )
        .init_git();
    let package_dir = context.child("prefix-fixture");

    let dependency = serde_json::to_string(package_dir.path())?;
    context.write_config(indoc::formatdoc! {r"
        repos:
          - repo: local
            hooks:
              - id: node
                name: node
                language: node
                entry: prek-prefix-fixture
                additional_dependencies: [{dependency}]
                always_run: true
                verbose: true
                pass_filenames: false
    "});

    context.git().add(".");

    let fake_prefix = context.home_dir().child("fake-prefix");
    fake_prefix.create_dir_all()?;
    let global_npmrc = fake_prefix.child("global-npmrc");
    let user_npmrc = fake_prefix.child("user-npmrc");
    global_npmrc.write_str("prefix=${HOME}/global-npmrc-prefix\n")?;
    user_npmrc.write_str("//registry.example.test/:_authToken=fake-token\n")?;

    cmd_snapshot!(context,
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
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r#"
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
    "#})
        .init_git();

    let new_path = remove_bin_from_path("node", None)?;

    cmd_snapshot!(context, context.run().env("PATH", new_path), @r"
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
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
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
    "})
        .init_git();

    let context = context.with_filter(r"\d+\.\d+\.\d+", "[NPM_VERSION]");

    cmd_snapshot!(context, context.run(), @r"
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

#[test]
fn node_install_preserves_global_git_config_and_isolates_repository() -> anyhow::Result<()> {
    let context = TestEnv::new().init_git();

    // Installing this additional dependency forces npm to invoke Git during environment setup.
    let dependency_repo = context.create_repo("sentinel-node-dependency").with_file(
        "package.json",
        indoc::indoc! {r#"
        {
          "name": "sentinel-node-dependency",
          "version": "1.0.0"
        }
    "#},
    );
    dependency_repo
        .git()
        .add(".")
        .commit("Add sentinel Node dependency");

    let hook_repo = context
        .create_hook_repo(
            "sentinel-node-hook",
            indoc::indoc! {r"
        - id: sentinel-node
          name: sentinel-node
          entry: sentinel-node-tool
          language: node
          always_run: true
          pass_filenames: false
    "},
        )
        .with_file(
            "package.json",
            indoc::indoc! {r#"
        {
          "name": "sentinel-node-tool",
          "version": "1.0.0",
          "bin": {
            "sentinel-node-tool": "cli.js"
          }
        }
    "#},
        )
        .with_executable_file(
            "cli.js",
            indoc::indoc! {r#"
        #!/usr/bin/env node
        console.log("sentinel node ok");
    "#},
        )
        .build();

    context.write_config(indoc::formatdoc! {r"
        repos:
          - repo: {repo}
            rev: v1.0.0
            hooks:
              - id: sentinel-node
                additional_dependencies:
                  - git+file:///prek-node-git-dependency
    ", repo = hook_repo});
    context.git().add(".");

    // The regression corrupts the calling repository's index, so capture it before npm runs.
    let staged_before = context
        .git()
        .command()
        .args(["ls-files", "--stage"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let dependency_url = Url::from_file_path(dependency_repo.path().path())
        .map_err(|()| anyhow::anyhow!("Failed to create dependency repository URL"))?;
    let global_gitconfig = context.child("global.gitconfig");
    // Keep the dependency local while requiring npm's Git subprocess to inherit global config.
    context
        .git()
        .command()
        .args(["config", "--file"])
        .arg(global_gitconfig.path())
        .arg(format!("url.{dependency_url}.insteadOf"))
        .arg("file:///prek-node-git-dependency")
        .assert()
        .success();

    let git_dir = context.child(".git");
    // Simulate the repository-local variables Git exports to hooks from a linked worktree.
    context
        .run()
        .arg("--all-files")
        .env(EnvVars::GIT_DIR, git_dir.path())
        .env("GIT_INDEX_FILE", git_dir.child("index").path())
        .env("GIT_CONFIG_GLOBAL", global_gitconfig.path())
        .env(EnvVars::GIT_TERMINAL_PROMPT, "0")
        .assert()
        .success();

    // Success proves the URL rewrite survived; an unchanged index proves repository isolation.
    let staged_after = context
        .git()
        .command()
        .args(["ls-files", "--stage"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(staged_after, staged_before);

    Ok(())
}
