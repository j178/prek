use prek_consts::env_vars::EnvVars;

use crate::common::{TestEnv, cmd_snapshot};

#[test]
fn local_hook() {
    let context = TestEnv::new_git()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: hello
                name: hello
                language: haskell
                entry: hello
                always_run: true
                verbose: true
                pass_filenames: false
    "})
        .with_file(
            "hello.cabal",
            indoc::indoc! {r"
            cabal-version:       3.0
            name:                hello
            version:             0.1.0.0
            build-type:          Simple

            executable hello
              main-is:             Main.hs
              default-language:    GHC2021
              build-depends:       base >= 4.19 && < 5
        "},
        )
        .with_file(
            "Main.hs",
            indoc::indoc! {r#"
            module Main where
            main :: IO ()
            main = putStrLn "Hello Haskell!"
        "#},
        );

    context.git().add_all();

    cmd_snapshot!(context, context.run().env(EnvVars::PREK_INTERNAL__SKIP_CABAL_UPDATE, "1"), @"
    success: true
    exit_code: 0
    ----- stdout -----
    hello....................................................................Passed
    - hook id: hello
    - duration: [TIME]

      Hello Haskell!

    ----- stderr -----
    ");

    // Run again to check `health_check` works correctly.
    cmd_snapshot!(context, context.run().env(EnvVars::PREK_INTERNAL__SKIP_CABAL_UPDATE, "1"), @"
    success: true
    exit_code: 0
    ----- stdout -----
    hello....................................................................Passed
    - hook id: hello
    - duration: [TIME]

      Hello Haskell!

    ----- stderr -----
    ");
}

#[test]
fn additional_dependencies() {
    let context = TestEnv::new_git().with_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: hello
                name: hello
                language: haskell
                entry: hello
                additional_dependencies: ["hello"]
                always_run: true
                verbose: true
                pass_filenames: false
    "#});

    context.git().add_all();

    cmd_snapshot!(context, context.run().env(EnvVars::PREK_INTERNAL__SKIP_CABAL_UPDATE, "1"), @"
    success: true
    exit_code: 0
    ----- stdout -----
    hello....................................................................Passed
    - hook id: hello
    - duration: [TIME]

      Hello, World!

    ----- stderr -----
    ");
}

#[test]
fn remote_hook() {
    let context = TestEnv::new_git().with_config(indoc::indoc! {r"
        repos:
          - repo: https://github.com/prek-ci/haskell-hooks
            rev: v1.0.0
            hooks:
              - id: hello
                always_run: true
                verbose: true
    "});

    context.git().add_all();

    cmd_snapshot!(context, context.run().env(EnvVars::PREK_INTERNAL__SKIP_CABAL_UPDATE, "1"), @"
    success: true
    exit_code: 0
    ----- stdout -----
    hello....................................................................Passed
    - hook id: hello
    - duration: [TIME]

      This is a remote haskell hook

    ----- stderr -----
    ");
}
