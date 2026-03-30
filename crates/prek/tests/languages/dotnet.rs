use assert_fs::fixture::{FileWriteStr, PathChild, PathCreateDir};
use prek_consts::PRE_COMMIT_HOOKS_YAML;
use prek_consts::env_vars::EnvVars;

use crate::common::{TestContext, cmd_snapshot, git_cmd};

#[test]
fn language_version() {
    if !EnvVars::is_set(EnvVars::CI) {
        return;
    }

    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              # `major.minor` channel request.
              - id: channel
                name: channel
                language: dotnet
                entry: dotnet --version
                language_version: '10.0'
                always_run: true
                verbose: true
                pass_filenames: false

              # Omit language_version to use the default SDK selection.
              - id: default
                name: default
                language: dotnet
                entry: dotnet --version
                always_run: true
                verbose: true
                pass_filenames: false

              # TFM-style request should resolve to the matching SDK channel.
              - id: tfm
                name: tfm
                language: dotnet
                entry: dotnet --version
                language_version: 'net10.0'
                always_run: true
                verbose: true
                pass_filenames: false

              # Major-only request should resolve to the latest matching channel.
              - id: major
                name: major
                language: dotnet
                entry: dotnet --version
                language_version: '10'
                always_run: true
                verbose: true
                pass_filenames: false
    "});

    context.git_add(".");

    let filters: Vec<_> = context
        .filters()
        .into_iter()
        .chain([(r"\b(\d+\.\d+)\.\d+\b", "$1.X")])
        .collect();

    cmd_snapshot!(filters, context.run(), @"
    success: true
    exit_code: 0
    ----- stdout -----
    channel..................................................................Passed
    - hook id: channel
    - duration: [TIME]

      10.0.X
    default..................................................................Passed
    - hook id: default
    - duration: [TIME]

      10.0.X
    tfm......................................................................Passed
    - hook id: tfm
    - duration: [TIME]

      10.0.X
    major....................................................................Passed
    - hook id: major
    - duration: [TIME]

      10.0.X

    ----- stderr -----
    ");
}

/// Test invalid `language_version` format is rejected.
#[test]
fn invalid_language_version() {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: local
                name: local
                language: dotnet
                entry: dotnet --version
                language_version: 'invalid-version'
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
      caused by: Invalid hook `local`
      caused by: Invalid `language_version` value: `invalid-version`
    ");
}

/// Test that multiple different SDK versions can coexist in the tool store.
/// `net10.0` is preinstalled in the CI, `net8.0` will be installed by the test.
#[test]
fn multiple_sdk_versions() -> anyhow::Result<()> {
    if !EnvVars::is_set(EnvVars::CI) {
        return Ok(());
    }

    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: hook-8
                name: hook-8
                language: dotnet
                entry: dotnet --version
                language_version: '8.0'
                always_run: true
                pass_filenames: false
                verbose: true
              - id: hook-10
                name: hook-10
                language: dotnet
                entry: dotnet --version
                language_version: '10.0'
                always_run: true
                pass_filenames: false
                verbose: true
    "});
    context.git_add(".");

    let filters: Vec<_> = context
        .filters()
        .into_iter()
        .chain([(r"\b(\d+\.\d+)\.\d+\b", "$1.X")])
        .collect();

    cmd_snapshot!(filters, context.run(), @"
    success: true
    exit_code: 0
    ----- stdout -----
    hook-8...................................................................Passed
    - hook id: hook-8
    - duration: [TIME]

      8.0.X
    hook-10..................................................................Passed
    - hook id: hook-10
    - duration: [TIME]

      10.0.X

    ----- stderr -----
    ");

    // Verify only net8.0 SDK is installed in the tool store since net10.0 is preinstalled in the CI environment.
    // Path structure: [HOME]/tools/dotnet/[VERSION]/...
    let dotnet_tool_root = context.home_dir().child("tools").child("dotnet");

    let mut found_8 = false;
    let mut found_10 = false;

    for entry in std::fs::read_dir(dotnet_tool_root.path())?.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('8') {
            found_8 = true;
        }
        if name.starts_with("10") {
            found_10 = true;
        }
    }

    assert!(found_8, "Managed dotnet 8.x should exist");
    assert!(
        !found_10,
        "dotnet 10.x should not be installed by the test since it's preinstalled in CI"
    );

    Ok(())
}

/// Test that `additional_dependencies` are installed correctly.
#[test]
fn additional_dependencies() {
    if !EnvVars::is_set(EnvVars::CI) {
        return;
    }

    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: local
                name: local
                language: dotnet
                entry: dotnet-outdated --version
                additional_dependencies: ["dotnet-outdated-tool"]
                always_run: true
                verbose: true
                pass_filenames: false
    "#});
    context.git_add(".");

    let filters: Vec<_> = context
        .filters()
        .into_iter()
        .chain([(r"\b(4\.7\.1\+)[0-9a-f]+\b", "${1}[SHA]")])
        .collect();

    cmd_snapshot!(filters.clone(), context.run(), @"
    success: true
    exit_code: 0
    ----- stdout -----
    local....................................................................Passed
    - hook id: local
    - duration: [TIME]

      A .NET Core global tool to list outdated Nuget packages.
      4.7.1+[SHA]

    ----- stderr -----
    ");

    // Run again to verify the `check_health` logic.
    cmd_snapshot!(filters, context.run(), @"
    success: true
    exit_code: 0
    ----- stdout -----
    local....................................................................Passed
    - hook id: local
    - duration: [TIME]

      A .NET Core global tool to list outdated Nuget packages.
      4.7.1+[SHA]

    ----- stderr -----
    ");
}

/// Test installing a specific version of a dotnet tool.
#[test]
fn additional_dependencies_with_version() {
    if !EnvVars::is_set(EnvVars::CI) {
        return;
    }

    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: local
                name: local
                language: dotnet
                entry: dotnet-outdated --version
                additional_dependencies: ["dotnet-outdated-tool:4.7.1"]
                always_run: true
                verbose: true
                pass_filenames: false
    "#});
    context.git_add(".");

    let filters: Vec<_> = context
        .filters()
        .into_iter()
        .chain([(r"\b(4\.7\.1\+)[0-9a-f]+\b", "${1}[SHA]")])
        .collect();

    cmd_snapshot!(filters, context.run(), @"
    success: true
    exit_code: 0
    ----- stdout -----
    local....................................................................Passed
    - hook id: local
    - duration: [TIME]

      A .NET Core global tool to list outdated Nuget packages.
      4.7.1+[SHA]

    ----- stderr -----
    ");
}

/// Test that additional dependencies in a remote repo are installed correctly.
#[test]
fn additional_dependencies_in_remote_repo() -> anyhow::Result<()> {
    if !EnvVars::is_set(EnvVars::CI) {
        return Ok(());
    }

    let repo = TestContext::new();
    repo.init_project();

    let repo_path = repo.work_dir();
    repo_path
        .child(PRE_COMMIT_HOOKS_YAML)
        .write_str(indoc::indoc! {r#"
        - id: dotnet-outdated
          name: dotnet-outdated
          language: dotnet
          entry: dotnet-outdated --version
          additional_dependencies: ["dotnet-outdated-tool:4.7.1"]
    "#})?;
    repo.git_add(".");
    repo.git_commit("Add manifest");
    git_cmd(repo.work_dir())
        .args(["tag", "v0.1.0", "-m", "v0.1.0"])
        .output()?;

    let context = TestContext::new();
    context.init_project();
    context.write_pre_commit_config(&indoc::formatdoc! {r"
        repos:
          - repo: {}
            rev: v0.1.0
            hooks:
              - id: dotnet-outdated
                verbose: true
                pass_filenames: false
    ", repo_path.display()});

    context.git_add(".");

    let filters: Vec<_> = context
        .filters()
        .into_iter()
        .chain([(r"\b(4\.7\.1\+)[0-9a-f]+\b", "${1}[SHA]")])
        .collect();

    cmd_snapshot!(filters, context.run(), @"
    success: true
    exit_code: 0
    ----- stdout -----
    dotnet-outdated..........................................................Passed
    - hook id: dotnet-outdated
    - duration: [TIME]

      A .NET Core global tool to list outdated Nuget packages.
      4.7.1+[SHA]

    ----- stderr -----
    ");

    Ok(())
}

/// Ensure that stderr from hooks is captured and shown to the user.
#[test]
fn hook_stderr() -> anyhow::Result<()> {
    if !EnvVars::is_set(EnvVars::CI) {
        return Ok(());
    }

    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: local
                name: local
                language: dotnet
                entry: dotnet run --project ./hook
    "});

    // Create a minimal console app that writes to stderr
    context.work_dir().child("hook").create_dir_all()?;
    context
        .work_dir()
        .child("hook/hook.csproj")
        .write_str(indoc::indoc! {r#"
        <Project Sdk="Microsoft.NET.Sdk">
          <PropertyGroup>
            <OutputType>Exe</OutputType>
            <TargetFramework>net10.0</TargetFramework>
            <ImplicitUsings>disable</ImplicitUsings>
          </PropertyGroup>
        </Project>
    "#})?;
    context
        .work_dir()
        .child("hook/Program.cs")
        .write_str(indoc::indoc! {r#"
        using System;
        Console.Error.WriteLine("Error from hook");
        Console.Error.Flush();
        Environment.Exit(1);
    "#})?;

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @"
    success: false
    exit_code: 1
    ----- stdout -----
    local....................................................................Failed
    - hook id: local
    - exit code: 1

      Error from hook

    ----- stderr -----
    ");

    Ok(())
}
