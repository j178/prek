#[cfg(feature = "ci")]
use anyhow::Result;
#[cfg(feature = "ci")]
use assert_fs::assert::PathAssert;
#[cfg(feature = "ci")]
use assert_fs::fixture::PathChild;
use prek_consts::env_vars::EnvVars;

use crate::common::{TestEnv, cmd_snapshot};

#[cfg(unix)]
mod system {
    use super::*;
    use anyhow::Result;
    use assert_fs::fixture::PathChild;

    fn context(request: &str) -> Result<(TestEnv, std::ffi::OsString)> {
        let context = TestEnv::new()
            .with_config(format!(
                r"
repos:
  - repo: local
    hooks:
      - id: rust
        name: rust
        language: rust
        language_version:
          request: '{request}'
          preference: only-system
        entry: cargo check
        always_run: true
        pass_filenames: false
"
            ))
            .with_executable_file(
                "rust/bin/cargo",
                "#!/bin/sh\nprintf 'compiler: '; \"$RUSTC\" --version && rustc --version >/dev/null\n",
            )
            .init_git();
        write_rustc(&context, "1.95.0");
        std::os::unix::fs::symlink(which::which("git")?, context.child("rust/bin/git"))?;
        let path = std::env::join_paths([context.child("rust/bin").path()])?;
        Ok((context, path))
    }

    fn write_rustc(context: &TestEnv, version: &str) {
        context.write_executable_file(
            "rust/bin/rustc",
            format!("#!/bin/sh\necho 'rustc {version}'\nif [ \"$1\" = -vV ]; then echo 'release: {version}'; fi\n"),
        );
    }

    #[test]
    fn standalone_toolchain_without_rustup() -> Result<()> {
        let (context, path) = context(">=1.95")?;

        cmd_snapshot!(context, context.run().arg("-v").env("PATH", &path).env("RUSTC", "/missing-rustc"), @r#"
        success: true
        exit_code: 0
        ----- stdout -----
        rust.....................................................................Passed
        - hook id: rust
        - duration: [TIME]

          compiler: rustc 1.95.0

        ----- stderr -----
        "#);
        assert!(!context.home_dir().child("tools/rustup").exists());
        Ok(())
    }

    #[test]
    fn requires_cargo_beside_rustc() -> Result<()> {
        let (context, _) = context("stable")?;
        fs_err::create_dir_all(context.child("cargo/bin"))?;
        fs_err::rename(
            context.child("rust/bin/cargo"),
            context.child("cargo/bin/cargo"),
        )?;
        context.git().add(".");
        let path = std::env::join_paths([
            context.child("rust/bin").path(),
            context.child("cargo/bin").path(),
        ])?;

        cmd_snapshot!(context, context.run().arg("-v").env("PATH", &path), @r#"
        success: false
        exit_code: 2
        ----- stdout -----

        ----- stderr -----
        error: Failed to install hook `rust`
          caused by: Failed to install rust
          caused by: No suitable Rust version found for toolchain policy: only-system (downloads disabled)
        "#);
        Ok(())
    }

    #[test]
    fn rustup_proxy_resolves_before_caching() -> Result<()> {
        let (context, _) = context("stable")?;
        context.write_executable_file(
            "proxy/rustup",
            indoc::indoc! {r#"
            #!/bin/sh
            if [ "$RUSTUP_TOOLCHAIN" = missing ]; then exit 1; fi
            case "$1 $2" in
                'which rustc') echo "$RUST_TEST_ROOT/rust/bin/rustc" ;;
                *) exit 1 ;;
            esac
        "#},
        );
        for name in ["rustc", "cargo"] {
            fs_err::hard_link(
                context.child("proxy/rustup"),
                context.child(format!("proxy/{name}")),
            )?;
        }
        let path = std::env::join_paths([
            context.child("proxy").path(),
            context.child("rust/bin").path(),
        ])?;
        cmd_snapshot!(context, context.run().arg("-v").env("PATH", &path).env("RUST_TEST_ROOT", context.work_dir().path()), @r#"
        success: true
        exit_code: 0
        ----- stdout -----
        rust.....................................................................Passed
        - hook id: rust
        - duration: [TIME]

          compiler: rustc 1.95.0

        ----- stderr -----
        "#);
        cmd_snapshot!(context, context.run().arg("-v").env("PATH", &path).env("RUSTUP_TOOLCHAIN", "missing"), @r#"
        success: true
        exit_code: 0
        ----- stdout -----
        rust.....................................................................Passed
        - hook id: rust
        - duration: [TIME]

          compiler: rustc 1.95.0

        ----- stderr -----
        "#);
        Ok(())
    }

    #[test]
    fn versioned_managed_toolchain_matches_channel() -> Result<()> {
        use assert_fs::fixture::PathCreateDir;

        let (context, path) = context("stable")?;
        context.write_config(
            context
                .read(".pre-commit-config.yaml")
                .replace("only-system", "only-managed"),
        );
        context.git().add(".pre-commit-config.yaml");
        let toolchain = context
            .home_dir()
            .child("tools/rustup/toolchains/1.95.0-test/bin");
        toolchain.create_dir_all()?;
        for name in ["rustc", "cargo"] {
            fs_err::copy(
                context.child(format!("rust/bin/{name}")),
                toolchain.child(name),
            )?;
        }
        write_rustc(&context, "1.99.0");
        cmd_snapshot!(context, context.run().arg("-v").env("PATH", &path), @r#"
        success: true
        exit_code: 0
        ----- stdout -----
        rust.....................................................................Passed
        - hook id: rust
        - duration: [TIME]

          compiler: rustc 1.95.0

        ----- stderr -----
        "#);
        Ok(())
    }

    #[cfg(feature = "ci")]
    #[test]
    fn installs_hook_with_standalone_compiler() -> Result<()> {
        use assert_cmd::assert::OutputAssertExt;

        let (context, _) = context("stable")?;
        for name in ["rustc", "cargo"] {
            let output = std::process::Command::new("rustup")
                .args(["which", name])
                .output()?
                .assert()
                .success();
            let executable = std::str::from_utf8(&output.get_output().stdout)?.trim();
            context.write_executable_file(
                format!("rust/bin/{name}"),
                format!("#!/bin/sh\nexec {} \"$@\"\n", shlex::try_quote(executable)?),
            );
        }
        let repo = context
            .create_hook_repo(
                "standalone",
                indoc::indoc! {r"
            - id: standalone
              name: standalone
              language: rust
              entry: standalone
              always_run: true
              pass_filenames: false
        "},
            )
            .with_file(
                "Cargo.toml",
                "[package]\nname = \"standalone\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
            )
            .with_file(
                "Cargo.lock",
                "version = 3\n[[package]]\nname = \"standalone\"\nversion = \"0.1.0\"\n",
            )
            .with_file(
                "src/main.rs",
                "fn main() { println!(\"built with system Rust\"); }\n",
            )
            .build();
        context.write_config(format!("repos:\n  - repo: {repo}\n    rev: v1.0.0\n    hooks:\n      - id: standalone\n        language_version:\n          preference: only-system\n"));
        context.git().add(".");
        let path = crate::common::remove_bin_from_path("rustc", None)?;
        let path = crate::common::remove_bin_from_path("cargo", Some(path))?;
        let path = crate::common::remove_bin_from_path("rustup", Some(path))?;
        let path = std::env::join_paths(
            std::iter::once(context.child("rust/bin").to_path_buf())
                .chain(std::env::split_paths(&path)),
        )?;
        for remove_cargo in [false, true] {
            if remove_cargo {
                fs_err::remove_file(context.child("rust/bin/cargo"))?;
                context.git().add(".");
            }
            insta::allow_duplicates! {
                cmd_snapshot!(context, context.run().arg("-v").env("PATH", &path).env("RUSTC", "/missing-rustc").env("CARGO_NET_OFFLINE", "true"), @r#"
        success: true
        exit_code: 0
        ----- stdout -----
        standalone...............................................................Passed
        - hook id: standalone
        - duration: [TIME]

          built with system Rust

        ----- stderr -----
        "#);
            }
        }
        assert!(
            !context.home_dir().child("tools/rustup").exists(),
            "unexpected rustup bootstrap"
        );
        Ok(())
    }
}

/// Test `language_version` parsing and installation for Rust hooks.
#[cfg(feature = "ci")]
#[test]
fn language_version() -> Result<()> {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: rust-system
                name: rust-system
                language: rust
                entry: rustc --version
                language_version: system
                pass_filenames: false
                always_run: true
              - id: rust-1.70 # should auto install 1.70.X
                name: rust-1.70
                language: rust
                entry: rustc --version
                language_version: '1.70'
                always_run: true
                pass_filenames: false
              - id: rust-1.70 # run again to ensure reusing the installed version
                name: rust-1.70
                language: rust
                entry: rustc --version
                language_version: '1.70'
                always_run: true
                pass_filenames: false
    "})
        .init_git();

    let rust_dir = context.home_dir().child("tools/rustup/toolchains");
    rust_dir.assert(predicates::path::missing());

    let context = context.with_filters([
        (r"rustc (1\.70)\.\d{1,2} .+", "rustc $1.X"), // Keep 1.70.X format
        (r"rustc 1\.\d{1,3}\.\d{1,2} .+", "rustc 1.X.X"), // Others become 1.X.X
    ]);

    cmd_snapshot!(context, context.run().arg("-v"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    rust-system..............................................................Passed
    - hook id: rust-system
    - duration: [TIME]

      rustc 1.X.X
    rust-1.70................................................................Passed
    - hook id: rust-1.70
    - duration: [TIME]

      rustc 1.70.X
    rust-1.70................................................................Passed
    - hook id: rust-1.70
    - duration: [TIME]

      rustc 1.70.X

    ----- stderr -----
    "#);

    // Ensure that only Rust 1.70.X is installed.
    let installed_versions = rust_dir
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
        "Expected only one Rust version to be installed, but found: {installed_versions:?}"
    );
    assert!(
        installed_versions.iter().any(|v| v.starts_with("1.70")),
        "Expected Rust 1.70.X to be installed, but found: {installed_versions:?}"
    );

    Ok(())
}

/// Test `rustup` installer.
#[test]
fn rustup_installer() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: rustup-test
                name: rustup-test
                language: rust
                language_version:
                  preference: only-managed
                entry: rustc --version
       "})
        .init_git();

    let context = context.with_filter(r"rustc 1\.\d{1,3}\.\d{1,2} .+", "rustc 1.X.X");

    cmd_snapshot!(context, context.run().arg("-v").env(EnvVars::PREK_INTERNAL__RUSTUP_BINARY_NAME, "non-exist-rustup"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    rustup-test..............................................................Passed
    - hook id: rustup-test
    - duration: [TIME]

      rustc 1.X.X

    ----- stderr -----
    "#);
}

/// Test that `additional_dependencies` with cli: prefix are installed correctly.
#[test]
fn additional_dependencies_cli() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: rust-cli
                name: rust-cli
                language: rust
                entry: prek-rust-echo Hello, Prek!
                additional_dependencies: ["cli:prek-rust-echo"]
                always_run: true
                verbose: true
                pass_filenames: false
    "#})
        .init_git();

    cmd_snapshot!(context, context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    rust-cli.................................................................Passed
    - hook id: rust-cli
    - duration: [TIME]

      Hello, Prek!

    ----- stderr -----
    ");
}

/// Test cargo-binstall is used only for crates.io CLI dependencies.
#[cfg(feature = "ci")]
#[test]
fn cargo_binstall_is_used_only_for_crates_io_cli_dependencies() {
    let context = TestEnv::new()
        .with_file(
            "Cargo.toml",
            indoc::indoc! {r#"
                [package]
                name = "cargo-binstall-test"
                version = "0.1.0"
                edition = "2021"
            "#},
        )
        .init_git();
    let hook_repo = context
        .create_hook_repo(
            "rust-cargo-hook",
            indoc::indoc! {r"
                - id: cargo-hook
                  name: cargo-hook
                  language: rust
                  entry: prek-cargo-hook
            "},
        )
        .with_file(
            "Cargo.toml",
            indoc::indoc! {r#"
                [package]
                name = "prek-cargo-hook"
                version = "0.1.0"
                edition = "2021"
            "#},
        )
        .with_file(
            "Cargo.lock",
            indoc::indoc! {r#"
                # This file is automatically @generated by Cargo.
                # It is not intended for manual editing.
                version = 4

                [[package]]
                name = "prek-cargo-hook"
                version = "0.1.0"
            "#},
        )
        .with_file(
            "src/main.rs",
            indoc::indoc! {r#"
                fn main() {
                    println!("installed from checkout");
                }
            "#},
        )
        .build();

    context.write_config(indoc::formatdoc! {r#"
        repos:
          - repo: {hook_repo}
            rev: v1.0.0
            hooks:
              - id: cargo-hook
                language_version: system
                always_run: true
                verbose: true
                pass_filenames: false
          - repo: local
            hooks:
              - id: binstall-cli
                name: binstall-cli
                language: rust
                entry: cargo-get package.name
                additional_dependencies: ["cli:cargo-get:1.4.0"]
                language_version: system
                always_run: true
                verbose: true
                pass_filenames: false
    "#});
    context.git().add(".");

    cmd_snapshot!(context, context.run()
        .env(EnvVars::PREK_USE_CARGO_BINSTALL, "1"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    cargo-hook...............................................................Passed
    - hook id: cargo-hook
    - duration: [TIME]

      installed from checkout
    binstall-cli.............................................................Passed
    - hook id: binstall-cli
    - duration: [TIME]

      cargo-binstall-test

    ----- stderr -----
    "#);
}

/// Test that remote Rust hooks are installed and run correctly.
#[test]
fn remote_hooks() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r#"
        repos:
          - repo: https://github.com/prek-ci/rust-hooks
            rev: v1.0.0
            hooks:
              - id: hello-world
                verbose: true
                pass_filenames: false
                always_run: true
                args: ["Hello World"]
    "#})
        .init_git();

    cmd_snapshot!(context, context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    Hello World..............................................................Passed
    - hook id: hello-world
    - description: echoes its arguments
    - duration: [TIME]

      Hello World

    ----- stderr -----
    ");
}

/// Test that remote Rust hooks from non-workspace repos are installed and run correctly.
#[test]
fn remote_hook_non_workspace() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: https://github.com/prek-ci/rust-hooks-non-workspace
            rev: v1.0.0
            hooks:
              - id: hello-world
                verbose: true
                pass_filenames: false
                always_run: true
    "})
        .init_git();

    cmd_snapshot!(context, context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    hello-world..............................................................Passed
    - hook id: hello-world
    - description: A simple hello world hook
    - duration: [TIME]

      Hello, Prek!

    ----- stderr -----
    ");
}

/// Test that library dependencies (non-cli: prefix) work correctly on remote hooks.
/// This verifies that the shared repo is not modified when adding dependencies.
#[test]
fn remote_hooks_with_lib_deps() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r#"
        repos:
          - repo: https://github.com/prek-ci/rust-hooks
            rev: v1.0.0
            hooks:
              - id: hello-world-lib-deps
                additional_dependencies: ["itoa:1"]
                verbose: true
                pass_filenames: false
                always_run: true
    "#})
        .init_git();

    cmd_snapshot!(context, context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    Hello World Lib Deps.....................................................Passed
    - hook id: hello-world-lib-deps
    - description: requires a library dependency to be added
    - duration: [TIME]

      42

    ----- stderr -----
    ");
}
