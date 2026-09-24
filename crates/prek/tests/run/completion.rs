use std::path::Path;

use anyhow::Result;
use assert_fs::prelude::*;
use prek_consts::PRE_COMMIT_CONFIG_YAML;

use crate::common::{TestEnv, cmd_snapshot};

fn write_project_config(path: &Path, hooks: &[(&str, &str)]) -> Result<()> {
    let mut yaml = String::from(indoc::indoc! {"
        repos:
          - repo: local
            hooks:
    "});
    for (id, name) in hooks {
        let hook = textwrap::indent(
            &indoc::formatdoc! {"
        - id: {}
          name: {}
          entry: echo
          language: system
        ", id, name
            },
            "      ",
        );
        yaml.push_str(&hook);
    }

    fs_err::create_dir_all(path)?;
    fs_err::write(path.join(PRE_COMMIT_CONFIG_YAML), yaml)?;

    Ok(())
}

#[cfg(unix)]
#[test]
fn selectors_completion() -> Result<()> {
    let context = TestEnv::new().init_git();
    let cwd = context.work_dir();

    // Root project with regular and colon-containing hook ids
    write_project_config(
        cwd,
        &[("root-hook", "Root Hook"), ("lint:ruff", "Ruff Lint")],
    )?;

    // Nested project at app/ with one hook
    let app = cwd.join("app");
    write_project_config(&app, &[("app-hook", "App Hook")])?;

    // Deeper nested project at app/lib/ with one hook
    let app_lib = app.join("lib");
    write_project_config(&app_lib, &[("lib-hook", "Lib Hook")])?;

    // Unrelated non-project dir should not appear in subdir suggestions
    context.child("scratch").create_dir_all()?;

    cmd_snapshot!(context, context.command().args(["util", "generate-shell-completion", "fish"]), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    complete --keep-order --exclusive --command prek --arguments "(COMPLETE=fish prek -- (commandline --current-process --tokenize --cut-at-cursor) (commandline --current-token))"

    ----- stderr -----
    "#);

    cmd_snapshot!(context, context.run().env("COMPLETE", "fish").arg("--").arg("prek").arg(""), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    init	Create a prek configuration and install Git hook shims
    install	Install prek Git hook shims
    prepare-hooks	Prepare environments for configured hooks
    run	Run configured hooks
    exec	Run a command in the environment prepared for a configured hook
    list	List configured hooks
    uninstall	Uninstall prek Git hook shims
    validate-config	Validate prek configuration files
    validate-manifest	Validate pre-commit hook manifests (`.pre-commit-hooks.yaml`)
    update	Update configured repositories
    cache	Manage the prek cache
    try-repo	Try hooks from a repository
    util	Run utility commands
    self	Manage the prek installation
    app/
    app:
    app-hook	App Hook
    lib-hook	Lib Hook
    :lint:ruff	Ruff Lint
    root-hook	Root Hook
    --skip	Skip the specified hooks or projects
    --stage	The stage during which the hook is fired
    --group	Run hooks belonging to the specified group
    --require-group	Run hooks belonging to every specified group
    --no-group	Do not run hooks belonging to the specified group
    --all-files	Run hooks on all tracked files in the repository
    --files	Run hooks on the specified file paths
    --glob	Run hooks on tracked files matching the specified glob pattern
    --directory	Run hooks on tracked files under the specified directory
    --from-ref	The original ref in a `<from_ref>...<to_ref>` diff expression. Files changed in this diff will be run through the hooks
    --to-ref	The destination ref in a `from_ref...to_ref` diff expression. Defaults to `HEAD` if `from_ref` is specified
    --last-commit	Run hooks against the last commit. Equivalent to `--from-ref HEAD~1 --to-ref HEAD`
    --show-diff-on-failure	When hooks fail, run `git diff` directly afterward
    --fail-fast	Stop running hooks after the first failure
    --dry-run	Do not run the hooks, but print the hooks that would have been run
    --hide-status	Hide hook reports with the specified final status
    --no-hide-status	Show all hook reports, overriding `hide_status` in configuration
    --config	Path to alternate config file
    --cd	Change to directory before running
    --color	Whether to use color in output
    --refresh	Refresh all cached data
    --help	Display the concise help for this command
    --no-progress	Hide all progress outputs
    --quiet	Use quiet output
    --verbose	Use verbose output
    --log-file	Write trace logs to the specified file. If not specified, trace logs will be written to `$PREK_HOME/prek.log`
    --version	Display the prek version

    ----- stderr -----
    "#);

    cmd_snapshot!(context, context.run().env("COMPLETE", "fish").arg("--").arg("prek").arg("."), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    ./
    .:

    ----- stderr -----
    ");

    cmd_snapshot!(context, context.run().env("COMPLETE", "fish").arg("--").arg("prek").arg("ap"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    app/
    app:
    app-hook	App Hook

    ----- stderr -----
    ");

    cmd_snapshot!(context, context.run().env("COMPLETE", "fish").arg("--").arg("prek").arg("app:"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    app:app-hook	App Hook

    ----- stderr -----
    ");

    cmd_snapshot!(context, context.run().env("COMPLETE", "fish").arg("--").arg("prek").arg("app:app"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    app:app-hook	App Hook

    ----- stderr -----
    ");

    cmd_snapshot!(context, context.run().env("COMPLETE", "fish").arg("--").arg("prek").arg("app/"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    app/lib/
    app/lib:

    ----- stderr -----
    ");
    cmd_snapshot!(context, context.run().env("COMPLETE", "fish").arg("--").arg("prek").arg("app/li"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    app/lib/
    app/lib:

    ----- stderr -----
    ");

    cmd_snapshot!(context, context.run().env("COMPLETE", "fish").arg("--").arg("prek").arg("app/lib:"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    app/lib:lib-hook	Lib Hook

    ----- stderr -----
    ");

    cmd_snapshot!(context, context.run().env("COMPLETE", "fish").arg("--").arg("prek").arg("app/lib/"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    app/lib/

    ----- stderr -----
    ");

    cmd_snapshot!(context, context.run().env("COMPLETE", "fish").arg("--").arg("prek").arg(".:root"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    .:root-hook	Root Hook

    ----- stderr -----
    ");

    cmd_snapshot!(context, context.run().env("COMPLETE", "fish").arg("--").arg("prek").arg(":lint:"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    :lint:ruff	Ruff Lint

    ----- stderr -----
    ");

    Ok(())
}
