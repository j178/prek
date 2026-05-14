use crate::common::{TestContext, cmd_snapshot};

mod common;

#[test]
fn cli_help() {
    let context = TestContext::new();
    cmd_snapshot!(context.filters(), context.command().arg("--help"), @"
    success: true
    exit_code: 0
    ----- stdout -----
    A fast Git hook manager written in Rust, designed as a drop-in alternative to pre-commit,
    reimagined.

    Usage: prek [OPTIONS] [HOOK|PROJECT]... [COMMAND]

    Commands:
      install            Install prek Git shims into Git's effective hooks directory
      prepare-hooks      Prepare environments for all hooks used in the config file
      run                Run hooks
      list               List hooks configured in the current workspace
      uninstall          Uninstall prek Git shims
      validate-config    Validate configuration files (prek.toml or .pre-commit-config.yaml)
      validate-manifest  Validate `.pre-commit-hooks.yaml` files
      sample-config      Produce a sample configuration file (prek.toml or .pre-commit-config.yaml)
      auto-update        Auto-update the `rev` field of repositories in the config file to the latest
                         version
      cache              Manage the prek cache
      try-repo           Try the pre-commit hooks in the current repo
      util               Utility commands
      self               `prek` self management

    Arguments:
      [HOOK|PROJECT]...  Include the specified hooks or projects

    Options:
          --skip <HOOK|PROJECT>   Skip the specified hooks or projects
      -a, --all-files             Run on all files in the repo
          --files [<FILES>...]    Specific filenames to run hooks on
      -d, --directory <DIR>       Run hooks on all files in the specified directories
      -s, --from-ref <FROM_REF>   The original ref in a `<from_ref>...<to_ref>` diff expression. Files
                                  changed in this diff will be run through the hooks
      -o, --to-ref <TO_REF>       The destination ref in a `from_ref...to_ref` diff expression. Defaults
                                  to `HEAD` if `from_ref` is specified
          --last-commit           Run hooks against the last commit. Equivalent to `--from-ref HEAD~1
                                  --to-ref HEAD`
          --stage <STAGE>         The stage during which the hook is fired [possible values: manual,
                                  commit-msg, post-checkout, post-commit, post-merge, post-rewrite,
                                  pre-commit, pre-merge-commit, pre-push, pre-rebase,
                                  prepare-commit-msg]
          --show-diff-on-failure  When hooks fail, run `git diff` directly afterward
          --fail-fast             Stop running hooks after the first failure
          --dry-run               Do not run the hooks, but print the hooks that would have been run

    Global options:
      -c, --config <CONFIG>      Path to alternate config file
      -C, --cd <DIR>             Change to directory before running
          --color <COLOR>        Whether to use color in output [env: PREK_COLOR=] [default: auto]
                                 [possible values: auto, always, never]
          --refresh              Refresh all cached data
      -h, --help                 Display the concise help for this command
          --no-progress          Hide all progress outputs
      -q, --quiet...             Use quiet output [env: PREK_QUIET=]
      -v, --verbose...           Use verbose output
          --log-file <LOG_FILE>  Write trace logs to the specified file. If not specified, trace logs
                                 will be written to `$PREK_HOME/prek.log`
      -V, --version              Display the prek version

    ----- stderr -----
    ");
}

#[test]
fn cli_version() {
    let context = TestContext::new();
    cmd_snapshot!(context.filters(), context.command().arg("--version"), @"
    success: true
    exit_code: 0
    ----- stdout -----
    prek 0.3.13+17 (a30dd721f 2026-05-14)

    ----- stderr -----
    ");
}
