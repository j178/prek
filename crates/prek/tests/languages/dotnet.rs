#[cfg(feature = "ci")]
use assert_fs::fixture::PathChild;

use crate::common::{TestEnv, cmd_snapshot};

#[cfg(feature = "ci")]
#[test]
fn language_version() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
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
    "})
        .init_git();

    let context = context.with_filter(r"\b(\d+\.\d+)\.\d+\b", "$1.X");

    cmd_snapshot!(context, context.run(), @"
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
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
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
    "})
        .init_git();

    cmd_snapshot!(context, context.run(), @r"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Failed to init hooks
      caused by: Invalid hook `local`
      caused by: Invalid `language_version` value: `invalid-version`
    ");
}

/// A managed .NET 8 SDK must coexist with the system SDK without downloading it.
#[cfg(feature = "ci")]
#[test]
fn multiple_sdk_versions() -> anyhow::Result<()> {
    let output = std::process::Command::new("dotnet")
        .arg("--version")
        .output()?;
    anyhow::ensure!(output.status.success(), "Failed to query the system SDK");
    let system_version = str::from_utf8(&output.stdout)?.trim();

    let context = TestEnv::new()
        .with_config(indoc::formatdoc! {r"
        repos:
          - repo: local
            hooks:
              - id: hook-8
                name: hook-8
                language: dotnet
                entry: dotnet --version
                language_version:
                  request: '8.0'
                  preference: only-managed
                always_run: true
                pass_filenames: false
                verbose: true
              - id: hook-system
                name: hook-system
                language: dotnet
                entry: dotnet --version
                language_version:
                  request: '{system_version}'
                  preference: only-system
                always_run: true
                pass_filenames: false
                verbose: true
    "})
        .init_git();

    let context = context
        .with_filter(regex::escape(system_version), "[SYSTEM_VERSION]")
        .with_filter(r"\b(\d+\.\d+)\.\d+\b", "$1.X");

    cmd_snapshot!(context, context.run(), @"
    success: true
    exit_code: 0
    ----- stdout -----
    hook-8...................................................................Passed
    - hook id: hook-8
    - duration: [TIME]

      8.0.X
    hook-system..............................................................Passed
    - hook id: hook-system
    - duration: [TIME]

      [SYSTEM_VERSION]

    ----- stderr -----
    ");

    let dotnet_tool_root = context.home_dir().child("tools").child("dotnet");

    let mut managed_versions = Vec::new();

    for entry in fs_err::read_dir(dotnet_tool_root.path())? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            let version = semver::Version::parse(&entry.file_name().to_string_lossy())?;
            managed_versions.push(version.major);
        }
    }

    assert_eq!(managed_versions, [8], "Only .NET 8 should be downloaded");

    Ok(())
}

/// Test installing a specific version of a dotnet tool.
#[cfg(feature = "ci")]
#[test]
fn additional_dependencies_with_version() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r#"
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
    "#})
        .init_git();

    let context = context.with_filter(r"\b(4\.7\.1\+)[0-9a-f]+\b", "${1}[SHA]");

    cmd_snapshot!(context, context.run(), @"
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
    cmd_snapshot!(context, context.run(), @"
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
#[cfg(feature = "ci")]
#[test]
fn additional_dependencies_in_remote_repo() {
    let context = TestEnv::new().init_git();
    let hook_repo = context
        .create_hook_repo(
            "dotnet-hook",
            indoc::indoc! {r#"
            - id: dotnet-outdated
              name: dotnet-outdated
              language: dotnet
              entry: dotnet-outdated --version
              additional_dependencies: ["dotnet-outdated-tool:4.7.1"]
        "#},
        )
        .build();

    context.write_config(indoc::formatdoc! {r"
        repos:
          - repo: {}
            rev: v1.0.0
            hooks:
              - id: dotnet-outdated
                verbose: true
                pass_filenames: false
    ", hook_repo});

    context.git().add(".");

    let context = context.with_filter(r"\b(4\.7\.1\+)[0-9a-f]+\b", "${1}[SHA]");

    cmd_snapshot!(context, context.run(), @"
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
}

/// Ensure that stderr from hooks is captured and shown to the user.
#[cfg(feature = "ci")]
#[test]
fn hook_stderr() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: local
                name: local
                language: dotnet
                entry: dotnet run --project ./hook
    "})
        .with_file(
            "hook/hook.csproj",
            indoc::indoc! {r#"
        <Project Sdk="Microsoft.NET.Sdk">
          <PropertyGroup>
            <OutputType>Exe</OutputType>
            <TargetFramework>net10.0</TargetFramework>
            <ImplicitUsings>disable</ImplicitUsings>
          </PropertyGroup>
        </Project>
    "#},
        )
        .with_file(
            "hook/Program.cs",
            indoc::indoc! {r#"
        using System;
        Console.Error.WriteLine("Error from hook");
        Console.Error.Flush();
        Environment.Exit(1);
    "#},
        )
        .init_git();

    cmd_snapshot!(context, context.run(), @"
    success: false
    exit_code: 1
    ----- stdout -----
    local....................................................................Failed
    - hook id: local
    - exit code: 1

      Error from hook

    ----- stderr -----
    ");
}
