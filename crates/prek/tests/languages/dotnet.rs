use assert_fs::fixture::{FileWriteStr, PathChild, PathCreateDir};
use prek_consts::PRE_COMMIT_HOOKS_YAML;

use crate::common::{TestContext, cmd_snapshot, git_cmd, remove_bin_from_path};

/// Test that `language_version` can specify a dotnet SDK version.
#[test]
fn language_version() {
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
                language_version: '8.0'
                always_run: true
                verbose: true
                pass_filenames: false
    "});

    context.git_add(".");

    let output = context.run().output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "hook should pass");
    assert!(
        stdout.contains("8.0"),
        "output should contain version 8.0, got: {stdout}"
    );
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

/// Test that `additional_dependencies` are installed correctly.
#[test]
fn additional_dependencies() {
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

    let output = context.run().output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "hook should pass");
    assert!(
        stdout.contains("dotnet-outdated") || stdout.contains("Nuget"),
        "output should mention the tool"
    );
}

/// Test installing a specific version of a dotnet tool.
#[test]
fn additional_dependencies_with_version() {
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
                additional_dependencies: ["dotnet-outdated-tool:4.6.0"]
                always_run: true
                verbose: true
                pass_filenames: false
    "#});

    context.git_add(".");

    let output = context.run().output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "hook should pass");
    assert!(
        stdout.contains("4.6.0"),
        "should install specific version 4.6.0"
    );
}

/// Test that additional dependencies in a remote repo are installed correctly.
#[test]
fn additional_dependencies_in_remote_repo() -> anyhow::Result<()> {
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
          additional_dependencies: ["dotnet-outdated-tool"]
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

    let output = context.run().output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "hook should pass");
    assert!(
        stdout.contains("dotnet-outdated") || stdout.contains("Nuget"),
        "output should mention the tool"
    );

    Ok(())
}

/// Ensure that stderr from hooks is captured and shown to the user.
#[test]
fn hook_stderr() -> anyhow::Result<()> {
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
          </PropertyGroup>
        </Project>
    "#})?;
    context
        .work_dir()
        .child("hook/Program.cs")
        .write_str(indoc::indoc! {r#"
        System.Console.Error.WriteLine("Error from hook");
        System.Environment.Exit(1);
    "#})?;

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
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

/// Test that `language_version: system` fails when no system dotnet is available.
#[test]
fn system_only_fails_without_dotnet() {
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
                language_version: system
                always_run: true
                pass_filenames: false
    "});

    context.git_add(".");

    // Remove dotnet from PATH to simulate missing system dotnet
    let path_without_dotnet =
        remove_bin_from_path("dotnet", None).expect("Failed to remove dotnet from PATH");

    cmd_snapshot!(context.filters(), context.run().env("PATH", &path_without_dotnet), @r"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Failed to install hook `local`
      caused by: Failed to install or find dotnet SDK
      caused by: No system dotnet installation found
    ");
}

/// Test that requesting an unavailable dotnet version fails gracefully.
#[test]
fn unavailable_version_fails() {
    let context = TestContext::new();
    context.init_project();

    // Request a very specific old version that won't exist
    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: local
                name: local
                language: dotnet
                entry: dotnet --version
                language_version: '1.0.0'
                always_run: true
                pass_filenames: false
    "});

    context.git_add(".");

    // Remove dotnet from PATH so it can't find system version
    let path_without_dotnet =
        remove_bin_from_path("dotnet", None).expect("Failed to remove dotnet from PATH");

    // This should fail because version 1.0.0 is ancient and won't be downloadable
    // via the modern install script
    let output = context
        .run()
        .env("PATH", &path_without_dotnet)
        .output()
        .unwrap();

    assert!(
        !output.status.success(),
        "should fail when requesting unavailable version"
    );
}

/// Test that default `language_version` works (uses system or downloads LTS).
#[test]
fn default_language_version() {
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
                always_run: true
                verbose: true
                pass_filenames: false
    "});

    context.git_add(".");

    let output = context.run().output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "hook should pass: {stdout}");
}

/// Test TFM-style version specification (net8.0, net9.0, etc.).
#[test]
fn tfm_style_language_version() {
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
                language_version: 'net8.0'
                always_run: true
                verbose: true
                pass_filenames: false
    "});

    context.git_add(".");

    let output = context.run().output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "hook should pass");
    assert!(
        stdout.contains("8.0"),
        "output should contain version 8.0, got: {stdout}"
    );
}

/// Test major-only version specification.
#[test]
fn major_only_language_version() {
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
                language_version: '8'
                always_run: true
                verbose: true
                pass_filenames: false
    "});

    context.git_add(".");

    let output = context.run().output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "hook should pass");
    assert!(
        stdout.contains("8."),
        "output should contain version 8.x, got: {stdout}"
    );
}

/// Test that `types: [c#]` filter correctly matches .cs files.
#[test]
fn csharp_type_filter() -> anyhow::Result<()> {
    let context = TestContext::new();
    context.init_project();

    context
        .work_dir()
        .child("Program.cs")
        .write_str("class Program { }")?;

    context
        .work_dir()
        .child("readme.txt")
        .write_str("This is a readme")?;

    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: csharp-echo
                name: csharp-echo
                language: system
                entry: "echo files:"
                types: [c#]
                verbose: true
    "#});

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    csharp-echo..............................................................Passed
    - hook id: csharp-echo
    - duration: [TIME]

      files: Program.cs

    ----- stderr -----
    ");

    Ok(())
}

/// Test that dotnet tools are installed in an isolated environment, not globally.
#[test]
fn tools_isolated_environment() -> anyhow::Result<()> {
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
                pass_filenames: false
    "#});

    context.git_add(".");

    let output = context.run().output().unwrap();
    assert!(output.status.success(), "hook should pass");

    // Verify the tool was installed in the prek hooks directory, not globally.
    // PREK_HOME is set to context.home_dir(), and hooks are stored in $PREK_HOME/hooks/
    let hooks_path = context.home_dir().child("hooks");

    // Find the dotnet environment directory
    let dotnet_env = std::fs::read_dir(hooks_path.path())?
        .flatten()
        .find(|entry| entry.file_name().to_string_lossy().starts_with("dotnet-"));

    assert!(
        dotnet_env.is_some(),
        "dotnet environment should exist in prek hooks directory"
    );

    let env_path = dotnet_env.unwrap().path();
    let tools_path = env_path.join("tools");

    assert!(
        tools_path.exists(),
        "tools directory should exist in isolated environment"
    );

    // Verify dotnet-outdated executable exists in the isolated tools path
    let tool_exists = std::fs::read_dir(&tools_path)?.flatten().any(|entry| {
        let name = entry.file_name().to_string_lossy().to_string();
        name.starts_with("dotnet-outdated")
    });

    assert!(
        tool_exists,
        "dotnet-outdated should be installed in isolated tools path: {}",
        tools_path.display()
    );

    Ok(())
}
