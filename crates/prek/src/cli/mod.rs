use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::builder::styling::{AnsiColor, Effects};
use clap::builder::{ArgPredicate, Styles};
use clap::{ArgAction, Args, Parser, Subcommand, ValueHint};
use clap_complete::engine::ArgValueCompleter;
use prek_consts::PRE_COMMIT_CONFIG_YAML;
use prek_consts::env_vars::EnvVars;
use serde::{Deserialize, Serialize};

use crate::config::{HookType, Language, Stage};

mod auto_update;
mod cache_clean;
mod cache_size;
mod completion;
mod hook_impl;
mod install;
mod list;
pub mod reporter;
pub mod run;
mod sample_config;
#[cfg(feature = "self-update")]
mod self_update;
mod try_repo;
mod validate;

pub(crate) use auto_update::auto_update;
pub(crate) use cache_clean::cache_clean;
pub(crate) use cache_size::cache_size;
use completion::selector_completer;
pub(crate) use hook_impl::hook_impl;
pub(crate) use install::{init_template_dir, install, install_hooks, uninstall};
pub(crate) use list::list;
pub(crate) use run::run;
pub(crate) use sample_config::sample_config;
#[cfg(feature = "self-update")]
pub(crate) use self_update::self_update;
pub(crate) use try_repo::try_repo;
pub(crate) use validate::{validate_configs, validate_manifest};

#[derive(Copy, Clone, PartialEq, Eq)]
pub(crate) enum ExitStatus {
    /// The command succeeded.
    Success,

    /// The command failed due to an error in the user input.
    Failure,

    /// The command failed with an unexpected error.
    Error,

    /// The command was interrupted.
    Interrupted,

    /// The command's exit status is propagated from an external command.
    External(u8),
}

impl From<ExitStatus> for ExitCode {
    fn from(status: ExitStatus) -> Self {
        match status {
            ExitStatus::Success => Self::from(0),
            ExitStatus::Failure => Self::from(1),
            ExitStatus::Error => Self::from(2),
            ExitStatus::Interrupted => Self::from(130),
            ExitStatus::External(code) => Self::from(code),
        }
    }
}

#[derive(Debug, Copy, Clone, clap::ValueEnum)]
pub enum ColorChoice {
    /// Enables colored output only when the output is going to a terminal or TTY with support.
    Auto,

    /// Enables colored output regardless of the detected environment.
    Always,

    /// Disables colored output.
    Never,
}

impl From<ColorChoice> for anstream::ColorChoice {
    fn from(value: ColorChoice) -> Self {
        match value {
            ColorChoice::Auto => Self::Auto,
            ColorChoice::Always => Self::Always,
            ColorChoice::Never => Self::Never,
        }
    }
}

const STYLES: Styles = Styles::styled()
    .header(AnsiColor::Green.on_default().effects(Effects::BOLD))
    .usage(AnsiColor::Green.on_default().effects(Effects::BOLD))
    .literal(AnsiColor::Cyan.on_default().effects(Effects::BOLD))
    .placeholder(AnsiColor::Cyan.on_default());

#[derive(Parser)]
#[command(
    name = "prek",
    long_version = crate::version::version(),
    about = "Better pre-commit, re-engineered in Rust"
)]
#[command(
    propagate_version = true,
    disable_help_flag = true,
    disable_help_subcommand = true,
    disable_version_flag = true
)]
#[command(styles=STYLES)]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: Option<Command>,

    // run as the default subcommand
    #[command(flatten)]
    pub(crate) run_args: RunArgs,

    #[command(flatten)]
    pub(crate) globals: GlobalArgs,
}

#[derive(Debug, Args)]
#[command(next_help_heading = "Global options", next_display_order = 1000)]
#[allow(clippy::struct_excessive_bools)]
pub(crate) struct GlobalArgs {
    /// Path to alternate config file.
    #[arg(global = true, short, long)]
    pub(crate) config: Option<PathBuf>,

    /// Change to directory before running.
    #[arg(
        global = true,
        short = 'C',
        long,
        value_name = "DIR",
        value_hint = ValueHint::DirPath,
    )]
    pub(crate) cd: Option<PathBuf>,

    /// Whether to use color in output.
    #[arg(
        global = true,
        long,
        value_enum,
        env = EnvVars::PREK_COLOR,
        default_value_t = ColorChoice::Auto,
    )]
    pub(crate) color: ColorChoice,

    /// Refresh all cached data.
    #[arg(global = true, long)]
    pub(crate) refresh: bool,

    /// Display the concise help for this command.
    #[arg(global = true, short, long, action = ArgAction::HelpShort)]
    help: (),

    /// Hide all progress outputs.
    ///
    /// For example, spinners or progress bars.
    #[arg(global = true, long)]
    pub no_progress: bool,

    /// Use quiet output.
    ///
    /// Repeating this option, e.g., `-qq`, will enable a silent mode in which
    /// prek will write no output to stdout.
    #[arg(global = true, short, long, conflicts_with = "verbose", action = ArgAction::Count)]
    pub quiet: u8,

    /// Use verbose output.
    #[arg(global = true, short, long, action = ArgAction::Count)]
    pub(crate) verbose: u8,

    /// Write trace logs to the specified file.
    /// If not specified, trace logs will be written to `$PREK_HOME/prek.log`.
    #[arg(global = true, long, value_name = "LOG_FILE", value_hint = ValueHint::FilePath)]
    pub(crate) log_file: Option<PathBuf>,

    /// Do not write trace logs to a log file.
    #[arg(global = true, long, overrides_with = "log_file", hide = true)]
    pub(crate) no_log_file: bool,

    /// Display the prek version.
    #[arg(global = true, short = 'V', long, action = ArgAction::Version)]
    version: (),

    /// Show the resolved settings for the current command.
    ///
    /// This option is used for debugging and development purposes.
    #[arg(global = true, long, hide = true)]
    pub show_settings: bool,
}

#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    /// Install the prek git hook.
    Install(InstallArgs),
    /// Create hook environments for all hooks used in the config file.
    ///
    /// This command does not install the git hook. To install the git hook along with the hook environments in one command, use `prek install --install-hooks`.
    InstallHooks(InstallHooksArgs),
    /// Run hooks.
    Run(Box<RunArgs>),
    /// List available hooks.
    List(ListArgs),
    /// Uninstall the prek git hook.
    Uninstall(UninstallArgs),
    /// Validate `.pre-commit-config.yaml` files.
    ValidateConfig(ValidateConfigArgs),
    /// Validate `.pre-commit-hooks.yaml` files.
    ValidateManifest(ValidateManifestArgs),
    /// Produce a sample `.pre-commit-config.yaml` file.
    SampleConfig(SampleConfigArgs),
    /// Auto-update pre-commit config to the latest repos' versions.
    #[command(alias = "autoupdate")]
    AutoUpdate(AutoUpdateArgs),
    /// Manage the prek cache.
    Cache(CacheNamespace),
    /// Clean unused cached repos.
    #[command(hide = true)]
    GC,
    /// Remove all prek cached data.
    #[command(hide = true)]
    Clean,
    /// Install hook script in a directory intended for use with `git config init.templateDir`.
    #[command(alias = "init-templatedir")]
    InitTemplateDir(InitTemplateDirArgs),
    /// Try the pre-commit hooks in the current repo.
    TryRepo(Box<TryRepoArgs>),
    /// The implementation of the `pre-commit` hook.
    #[command(hide = true)]
    HookImpl(HookImplArgs),
    /// `prek` self management.
    #[command(name = "self")]
    Self_(SelfNamespace),
    /// Generate shell completion scripts.
    #[command(hide = true)]
    GenerateShellCompletion(GenerateShellCompletionArgs),
}

#[derive(Debug, Args)]
pub(crate) struct InstallArgs {
    /// Include the specified hooks or projects.
    ///
    /// Supports flexible selector syntax:
    ///
    /// - `hook-id`: Run all hooks with the specified ID across all projects
    ///
    /// - `project-path/`: Run all hooks from the specified project
    ///
    /// - `project-path:hook-id`: Run only the specified hook from the specified project
    ///
    /// Can be specified multiple times to select multiple hooks/projects.
    #[arg(
        value_name = "HOOK|PROJECT",
        value_hint = ValueHint::Other,
        add = ArgValueCompleter::new(selector_completer)
    )]
    pub(crate) includes: Vec<String>,

    /// Skip the specified hooks or projects.
    ///
    /// Supports flexible selector syntax:
    ///
    /// - `hook-id`: Skip all hooks with the specified ID across all projects
    ///
    /// - `project-path/`: Skip all hooks from the specified project
    ///
    /// - `project-path:hook-id`: Skip only the specified hook from the specified project
    ///
    /// Can be specified multiple times. Also accepts `PREK_SKIP` or `SKIP` environment variables (comma-delimited).
    #[arg(long = "skip", value_name = "HOOK|PROJECT", add = ArgValueCompleter::new(selector_completer))]
    pub(crate) skips: Vec<String>,

    /// Overwrite existing hooks.
    #[arg(short = 'f', long)]
    pub(crate) overwrite: bool,

    /// Create hook environments for all hooks used in the config file.
    #[arg(long)]
    pub(crate) install_hooks: bool,

    /// Which hook type(s) to install.
    ///
    /// Specifies which git hook stage(s) you want to install the hook script for.
    /// Can be specified multiple times to install hooks for multiple stages.
    ///
    /// If not specified, uses `default_install_hook_types` from the config file,
    /// or defaults to `pre-commit` if that is also not set.
    ///
    /// Note: This is different from a hook's `stages` parameter in the config file,
    /// which declares which stages a hook *can* run in.
    #[arg(short = 't', long = "hook-type", value_name = "HOOK_TYPE", value_enum)]
    pub(crate) hook_types: Vec<HookType>,

    /// Allow a missing `pre-commit` configuration file.
    #[arg(long)]
    pub(crate) allow_missing_config: bool,
}

#[derive(Debug, Args)]
pub(crate) struct InstallHooksArgs {
    /// Include the specified hooks or projects.
    ///
    /// Supports flexible selector syntax:
    ///
    /// - `hook-id`: Run all hooks with the specified ID across all projects
    ///
    /// - `project-path/`: Run all hooks from the specified project
    ///
    /// - `project-path:hook-id`: Run only the specified hook from the specified project
    ///
    /// Can be specified multiple times to select multiple hooks/projects.
    #[arg(
        value_name = "HOOK|PROJECT",
        value_hint = ValueHint::Other,
        add = ArgValueCompleter::new(selector_completer)
    )]
    pub(crate) includes: Vec<String>,

    /// Skip the specified hooks or projects.
    ///
    /// Supports flexible selector syntax:
    ///
    /// - `hook-id`: Skip all hooks with the specified ID across all projects
    ///
    /// - `project-path/`: Skip all hooks from the specified project
    ///
    /// - `project-path:hook-id`: Skip only the specified hook from the specified project
    ///
    /// Can be specified multiple times. Also accepts `PREK_SKIP` or `SKIP` environment variables (comma-delimited).
    #[arg(long = "skip", value_name = "HOOK|PROJECT", add = ArgValueCompleter::new(selector_completer))]
    pub(crate) skips: Vec<String>,
}

#[derive(Debug, Args)]
pub(crate) struct UninstallArgs {
    /// Which hook type(s) to uninstall.
    ///
    /// Specifies which git hook stage(s) you want to uninstall.
    /// Can be specified multiple times to uninstall hooks for multiple stages.
    ///
    /// If not specified, uses `default_install_hook_types` from the config file,
    /// or defaults to `pre-commit` if that is also not set.
    #[arg(short = 't', long = "hook-type", value_name = "HOOK_TYPE", value_enum)]
    pub(crate) hook_types: Vec<HookType>,
}

#[derive(Debug, Clone, Default, Args)]
pub(crate) struct RunExtraArgs {
    #[arg(long, hide = true)]
    pub(crate) remote_branch: Option<String>,
    #[arg(long, hide = true)]
    pub(crate) local_branch: Option<String>,
    #[arg(long, hide = true, required_if_eq("hook_stage", "pre-rebase"))]
    pub(crate) pre_rebase_upstream: Option<String>,
    #[arg(long, hide = true)]
    pub(crate) pre_rebase_branch: Option<String>,
    #[arg(long, hide = true, required_if_eq_any = [("hook_stage", "prepare-commit-msg"), ("hook_stage", "commit-msg")])]
    pub(crate) commit_msg_filename: Option<String>,
    #[arg(long, hide = true)]
    pub(crate) prepare_commit_message_source: Option<String>,
    #[arg(long, hide = true)]
    pub(crate) commit_object_name: Option<String>,
    #[arg(long, hide = true)]
    pub(crate) remote_name: Option<String>,
    #[arg(long, hide = true)]
    pub(crate) remote_url: Option<String>,
    #[arg(long, hide = true)]
    pub(crate) checkout_type: Option<String>,
    #[arg(long, hide = true)]
    pub(crate) is_squash_merge: bool,
    #[arg(long, hide = true)]
    pub(crate) rewrite_command: Option<String>,
}

#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Default, Args)]
pub(crate) struct RunArgs {
    /// Include the specified hooks or projects.
    ///
    /// Supports flexible selector syntax:
    ///
    /// - `hook-id`: Run all hooks with the specified ID across all projects
    ///
    /// - `project-path/`: Run all hooks from the specified project
    ///
    /// - `project-path:hook-id`: Run only the specified hook from the specified project
    ///
    /// Can be specified multiple times to select multiple hooks/projects.
    #[arg(
        value_name = "HOOK|PROJECT",
        value_hint = ValueHint::Other,
        add = ArgValueCompleter::new(selector_completer)
    )]
    pub(crate) includes: Vec<String>,

    /// Skip the specified hooks or projects.
    ///
    /// Supports flexible selector syntax:
    ///
    /// - `hook-id`: Skip all hooks with the specified ID across all projects
    ///
    /// - `project-path/`: Skip all hooks from the specified project
    ///
    /// - `project-path:hook-id`: Skip only the specified hook from the specified project
    ///
    /// Can be specified multiple times. Also accepts `PREK_SKIP` or `SKIP` environment variables (comma-delimited).
    #[arg(long = "skip", value_name = "HOOK|PROJECT", add = ArgValueCompleter::new(selector_completer))]
    pub(crate) skips: Vec<String>,

    /// Run on all files in the repo.
    #[arg(short, long, conflicts_with_all = ["files", "from_ref", "to_ref"])]
    pub(crate) all_files: bool,
    /// Specific filenames to run hooks on.
    #[arg(
        long,
        conflicts_with_all = ["all_files", "from_ref", "to_ref"],
        num_args = 0..,
        value_hint = ValueHint::AnyPath)
    ]
    pub(crate) files: Vec<String>,

    /// Run hooks on all files in the specified directories.
    ///
    /// You can specify multiple directories. It can be used in conjunction with `--files`.
    #[arg(
        short,
        long,
        value_name = "DIR",
        conflicts_with_all = ["all_files", "from_ref", "to_ref"],
        value_hint = ValueHint::DirPath
    )]
    pub(crate) directory: Vec<String>,

    /// The original ref in a `<from_ref>...<to_ref>` diff expression.
    /// Files changed in this diff will be run through the hooks.
    #[arg(short = 's', long, alias = "source", value_hint = ValueHint::Other)]
    pub(crate) from_ref: Option<String>,

    /// The destination ref in a `from_ref...to_ref` diff expression.
    /// Defaults to `HEAD` if `from_ref` is specified.
    #[arg(
        short = 'o',
        long,
        alias = "origin",
        requires = "from_ref",
        value_hint = ValueHint::Other,
        default_value_if("from_ref", ArgPredicate::IsPresent, "HEAD")
    )]
    pub(crate) to_ref: Option<String>,

    /// Run hooks against the last commit. Equivalent to `--from-ref HEAD~1 --to-ref HEAD`.
    #[arg(long, conflicts_with_all = ["all_files", "files", "directory", "from_ref", "to_ref"])]
    pub(crate) last_commit: bool,

    /// The stage during which the hook is fired.
    ///
    /// When specified, only hooks configured for that stage (for example `manual`,
    /// `pre-commit`, or `pre-commit`) will run.
    /// Defaults to `pre-commit` if not specified.
    /// For hooks specified directly in the command line, fallback to `manual` stage if no hooks found for `pre-commit` stage.
    #[arg(long, value_enum)]
    pub(crate) hook_stage: Option<Stage>,

    /// When hooks fail, run `git diff` directly afterward.
    #[arg(long)]
    pub(crate) show_diff_on_failure: bool,

    /// Stop running hooks after the first failure.
    #[arg(long)]
    pub(crate) fail_fast: bool,

    /// Do not run the hooks, but print the hooks that would have been run.
    #[arg(long)]
    pub(crate) dry_run: bool,

    #[command(flatten)]
    pub(crate) extra: RunExtraArgs,
}

#[derive(Debug, Clone, Default, Args)]
pub(crate) struct TryRepoArgs {
    /// Repository to source hooks from.
    pub(crate) repo: String,

    /// Manually select a rev to run against, otherwise the `HEAD` revision will be used.
    #[arg(long, alias = "ref")]
    pub(crate) rev: Option<String>,

    #[command(flatten)]
    pub(crate) run_args: RunArgs,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ListOutputFormat {
    #[default]
    Text,
    Json,
}

#[derive(Debug, Clone, Default, Args)]
pub(crate) struct ListArgs {
    /// Include the specified hooks or projects.
    ///
    /// Supports flexible selector syntax:
    ///
    /// - `hook-id`: Run all hooks with the specified ID across all projects
    ///
    /// - `project-path/`: Run all hooks from the specified project
    ///
    /// - `project-path:hook-id`: Run only the specified hook from the specified project
    ///
    /// Can be specified multiple times to select multiple hooks/projects.
    #[arg(
        value_name = "HOOK|PROJECT",
        value_hint = ValueHint::Other,
        add = ArgValueCompleter::new(selector_completer)
    )]
    pub(crate) includes: Vec<String>,

    /// Skip the specified hooks or projects.
    ///
    /// Supports flexible selector syntax:
    ///
    /// - `hook-id`: Skip all hooks with the specified ID across all projects
    ///
    /// - `project-path/`: Skip all hooks from the specified project
    ///
    /// - `project-path:hook-id`: Skip only the specified hook from the specified project
    ///
    /// Can be specified multiple times. Also accepts `PREK_SKIP` or `SKIP` environment variables (comma-delimited).
    #[arg(long = "skip", value_name = "HOOK|PROJECT", add = ArgValueCompleter::new(selector_completer))]
    pub(crate) skips: Vec<String>,

    /// Show only hooks that has the specified stage.
    #[arg(long, value_enum)]
    pub(crate) hook_stage: Option<Stage>,
    /// Show only hooks that are implemented in the specified language.
    #[arg(long, value_enum)]
    pub(crate) language: Option<Language>,
    /// The output format.
    #[arg(long, value_enum, default_value_t = ListOutputFormat::Text)]
    pub(crate) output_format: ListOutputFormat,
}

#[derive(Debug, Args)]
pub(crate) struct ValidateConfigArgs {
    /// The path to the configuration file.
    #[arg(value_name = "CONFIG")]
    pub(crate) configs: Vec<PathBuf>,
}

#[derive(Debug, Args)]
pub(crate) struct ValidateManifestArgs {
    /// The path to the manifest file.
    #[arg(value_name = "MANIFEST")]
    pub(crate) manifests: Vec<PathBuf>,
}

#[derive(Debug, Args)]
pub(crate) struct SampleConfigArgs {
    /// Write the sample config to a file (`.pre-commit-config.yaml` by default).
    #[arg(
        short,
        long,
        num_args = 0..=1,
        default_missing_value = PRE_COMMIT_CONFIG_YAML,
    )]
    pub(crate) file: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub(crate) struct AutoUpdateArgs {
    /// Update to the bleeding edge of the default branch instead of the latest tagged version.
    #[arg(long)]
    pub(crate) bleeding_edge: bool,
    /// Store "frozen" hashes in `rev` instead of tag names.
    #[arg(long)]
    pub(crate) freeze: bool,
    /// Only update this repository. This option may be specified multiple times.
    #[arg(long)]
    pub(crate) repo: Vec<String>,
    /// Do not write changes to the config file, only display what would be changed.
    #[arg(long)]
    pub(crate) dry_run: bool,
    /// Number of threads to use.
    #[arg(short, long, default_value_t = 0)]
    pub(crate) jobs: usize,
    /// Minimum release age (in days) required for a version to be eligible.
    ///
    /// The age is computed from the tag creation timestamp for annotated tags, or from the tagged commit timestamp for lightweight tags.
    /// A value of `0` disables this check.
    #[arg(
        long,
        value_name = "DAYS",
        default_value_t = 0,
        conflicts_with = "bleeding_edge"
    )]
    pub(crate) cooldown_days: u8,
}

#[derive(Debug, Args)]
pub(crate) struct HookImplArgs {
    /// Include the specified hooks or projects.
    ///
    /// Supports flexible selector syntax:
    ///
    /// - `hook-id`: Run all hooks with the specified ID across all projects
    ///
    /// - `project-path/`: Run all hooks from the specified project
    ///
    /// - `project-path:hook-id`: Run only the specified hook from the specified project
    ///
    /// Can be specified multiple times to select multiple hooks/projects.
    #[arg(
        value_name = "HOOK|PROJECT",
        value_hint = ValueHint::Other,
        add = ArgValueCompleter::new(selector_completer)
    )]
    pub(crate) includes: Vec<String>,

    /// Skip the specified hooks or projects.
    ///
    /// Supports flexible selector syntax:
    ///
    /// - `hook-id`: Skip all hooks with the specified ID across all projects
    ///
    /// - `project-path/`: Skip all hooks from the specified project
    ///
    /// - `project-path:hook-id`: Skip only the specified hook from the specified project
    ///
    /// Can be specified multiple times. Also accepts `PREK_SKIP` or `SKIP` environment variables (comma-delimited).
    #[arg(long = "skip", value_name = "HOOK|PROJECT", add = ArgValueCompleter::new(selector_completer))]
    pub(crate) skips: Vec<String>,
    #[arg(long)]
    pub(crate) hook_type: HookType,
    #[arg(long)]
    pub(crate) hook_dir: PathBuf,
    #[arg(long)]
    pub(crate) skip_on_missing_config: bool,
    /// The prek version that installs the hook.
    #[arg(long)]
    pub(crate) script_version: Option<usize>,
    #[arg(last = true)]
    pub(crate) args: Vec<OsString>,
}

#[derive(Debug, Args)]
pub(crate) struct CacheNamespace {
    #[command(subcommand)]
    pub(crate) command: CacheCommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum CacheCommand {
    /// Show the location of the prek cache.
    Dir,
    /// Remove unused cached repositories, hook environments, and other data.
    GC,
    /// Remove all prek cached data.
    Clean,
    /// Show the size of the prek cache.
    Size(SizeArgs),
}

#[derive(Args, Debug)]
pub struct SizeArgs {
    /// Display the cache size in human-readable format (e.g., `1.2 GiB` instead of raw bytes).
    #[arg(long = "human", short = 'H', alias = "human-readable")]
    pub(crate) human: bool,
}

#[derive(Debug, Args)]
pub(crate) struct SelfNamespace {
    #[command(subcommand)]
    pub(crate) command: SelfCommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum SelfCommand {
    /// Update prek.
    Update(SelfUpdateArgs),
}

#[derive(Debug, Args)]
pub(crate) struct SelfUpdateArgs {
    /// Update to the specified version.
    /// If not provided, prek will update to the latest version.
    pub target_version: Option<String>,

    /// A GitHub token for authentication.
    /// A token is not required but can be used to reduce the chance of encountering rate limits.
    #[arg(long, env = "GITHUB_TOKEN")]
    pub token: Option<String>,
}

#[derive(Debug, Args)]
pub(crate) struct GenerateShellCompletionArgs {
    /// The shell to generate the completion script for
    #[arg(value_enum)]
    pub shell: clap_complete::Shell,
}

#[derive(Debug, Args)]
pub(crate) struct InitTemplateDirArgs {
    /// The directory in which to write the hook script.
    pub(crate) directory: PathBuf,

    /// Assume cloned repos should have a `pre-commit` config.
    #[arg(long)]
    pub(crate) no_allow_missing_config: bool,

    /// Which hook type(s) to install.
    ///
    /// Specifies which git hook stage(s) you want to install the hook script for.
    /// Can be specified multiple times to install hooks for multiple stages.
    ///
    /// If not specified, uses `default_install_hook_types` from the config file,
    /// or defaults to `pre-commit` if that is also not set.
    #[arg(short = 't', long = "hook-type", value_name = "HOOK_TYPE", value_enum)]
    pub(crate) hook_types: Vec<HookType>,
}

#[cfg(unix)]
#[cfg(test)]
mod _gen {
    use crate::cli::Cli;
    use anyhow::{Result, bail};
    use clap::{Command, CommandFactory};
    use itertools::Itertools;
    use prek_consts::env_vars::EnvVars;
    use pretty_assertions::StrComparison;
    use std::cmp::max;
    use std::path::PathBuf;

    const ROOT_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../");

    enum Mode {
        /// Update the content.
        Write,

        /// Don't write to the file, check if the file is up-to-date and error if not.
        Check,

        /// Write the generated help to stdout.
        DryRun,
    }

    fn generate(mut cmd: Command) -> String {
        let mut output = String::new();

        cmd.build();

        let mut parents = Vec::new();

        output.push_str("# CLI Reference\n\n");
        generate_command(&mut output, &cmd, &mut parents);

        output
    }

    #[allow(clippy::format_push_string)]
    fn generate_command<'a>(
        output: &mut String,
        command: &'a Command,
        parents: &mut Vec<&'a Command>,
    ) {
        if command.is_hide_set() {
            return;
        }

        // Generate the command header.
        let name = if parents.is_empty() {
            command.get_name().to_string()
        } else {
            format!(
                "{} {}",
                parents.iter().map(|cmd| cmd.get_name()).join(" "),
                command.get_name()
            )
        };

        // Display the top-level `prek` command at the same level as its children
        let level = max(2, parents.len() + 1);
        output.push_str(&format!("{} {name}\n\n", "#".repeat(level)));

        // Display the command description.
        if let Some(about) = command.get_long_about().or_else(|| command.get_about()) {
            output.push_str(&about.to_string());
            output.push_str("\n\n");
        }

        // Display the usage
        {
            // This appears to be the simplest way to get rendered usage from Clap,
            // it is complicated to render it manually. It's annoying that it
            // requires a mutable reference but it doesn't really matter.
            let mut command = command.clone();
            output.push_str("<h3 class=\"cli-reference\">Usage</h3>\n\n");
            output.push_str(&format!(
                "```\n{}\n```",
                command
                    .render_usage()
                    .to_string()
                    .trim_start_matches("Usage: "),
            ));
            output.push_str("\n\n");
        }

        // Display a list of child commands
        let mut subcommands = command.get_subcommands().peekable();
        let has_subcommands = subcommands.peek().is_some();
        if has_subcommands {
            output.push_str("<h3 class=\"cli-reference\">Commands</h3>\n\n");
            output.push_str("<dl class=\"cli-reference\">");

            for subcommand in subcommands {
                if subcommand.is_hide_set() {
                    continue;
                }
                let subcommand_name = format!("{name} {}", subcommand.get_name());
                output.push_str(&format!(
                    "<dt><a href=\"#{}\"><code>{subcommand_name}</code></a></dt>",
                    subcommand_name.replace(' ', "-")
                ));
                if let Some(about) = subcommand.get_about() {
                    output.push_str(&format!(
                        "<dd>{}</dd>\n",
                        markdown::to_html(&about.to_string())
                    ));
                }
            }

            output.push_str("</dl>\n\n");
        }

        // Do not display options for commands with children
        if !has_subcommands {
            let name_key = name.replace(' ', "-");

            // Display positional arguments
            let mut arguments = command
                .get_positionals()
                .filter(|arg| !arg.is_hide_set())
                .peekable();

            if arguments.peek().is_some() {
                output.push_str("<h3 class=\"cli-reference\">Arguments</h3>\n\n");
                output.push_str("<dl class=\"cli-reference\">");

                for arg in arguments {
                    let id = format!("{name_key}--{}", arg.get_id());
                    output.push_str(&format!("<dt id=\"{id}\">"));
                    output.push_str(&format!(
                        "<a href=\"#{id}\"<code>{}</code></a>",
                        arg.get_value_names()
                            .unwrap()
                            .iter()
                            .next()
                            .unwrap()
                            .to_string()
                            .to_uppercase(),
                    ));
                    output.push_str("</dt>");
                    if let Some(help) = arg.get_long_help().or_else(|| arg.get_help()) {
                        output.push_str("<dd>");
                        output.push_str(&format!("{}\n", markdown::to_html(&help.to_string())));
                        output.push_str("</dd>");
                    }
                }

                output.push_str("</dl>\n\n");
            }

            // Display options and flags
            let mut options = command
                .get_arguments()
                .filter(|arg| !arg.is_positional())
                .filter(|arg| !arg.is_hide_set())
                .sorted_by_key(|arg| arg.get_id())
                .peekable();

            if options.peek().is_some() {
                output.push_str("<h3 class=\"cli-reference\">Options</h3>\n\n");
                output.push_str("<dl class=\"cli-reference\">");
                for opt in options {
                    let Some(long) = opt.get_long() else { continue };
                    let id = format!("{name_key}--{long}");

                    output.push_str(&format!("<dt id=\"{id}\">"));
                    output.push_str(&format!("<a href=\"#{id}\"><code>--{long}</code></a>"));
                    for long_alias in opt.get_all_aliases().into_iter().flatten() {
                        output.push_str(&format!(", <code>--{long_alias}</code>"));
                    }
                    if let Some(short) = opt.get_short() {
                        output.push_str(&format!(", <code>-{short}</code>"));
                    }
                    for short_alias in opt.get_all_short_aliases().into_iter().flatten() {
                        output.push_str(&format!(", <code>-{short_alias}</code>"));
                    }

                    // Re-implements private `Arg::is_takes_value_set` used in `Command::get_opts`
                    if opt
                        .get_num_args()
                        .unwrap_or_else(|| 1.into())
                        .takes_values()
                    {
                        if let Some(values) = opt.get_value_names() {
                            for value in values {
                                output.push_str(&format!(
                                    " <i>{}</i>",
                                    value.to_lowercase().replace('_', "-")
                                ));
                            }
                        }
                    }
                    output.push_str("</dt>");
                    if let Some(help) = opt.get_long_help().or_else(|| opt.get_help()) {
                        output.push_str("<dd>");
                        output.push_str(&format!("{}\n", markdown::to_html(&help.to_string())));
                        emit_env_option(opt, output);
                        emit_default_option(opt, output);
                        emit_possible_options(opt, output);
                        output.push_str("</dd>");
                    }
                }

                output.push_str("</dl>");
            }

            output.push_str("\n\n");
        }

        parents.push(command);

        // Recurse to all the subcommands.
        for subcommand in command.get_subcommands() {
            generate_command(output, subcommand, parents);
        }

        parents.pop();
    }

    fn emit_env_option(opt: &clap::Arg, output: &mut String) {
        if opt.is_hide_env_set() {
            return;
        }
        if let Some(env) = opt.get_env() {
            output.push_str(&markdown::to_html(&format!(
                "May also be set with the `{}` environment variable.",
                env.to_string_lossy()
            )));
        }
    }

    fn emit_default_option(opt: &clap::Arg, output: &mut String) {
        if opt.is_hide_default_value_set() || !opt.get_num_args().expect("built").takes_values() {
            return;
        }

        let values = opt.get_default_values();
        if !values.is_empty() {
            let value = format!(
                "\n[default: {}]",
                opt.get_default_values()
                    .iter()
                    .map(|s| s.to_string_lossy())
                    .join(",")
            );
            output.push_str(&markdown::to_html(&value));
        }
    }

    fn emit_possible_options(opt: &clap::Arg, output: &mut String) {
        if opt.is_hide_possible_values_set() {
            return;
        }

        let values = opt.get_possible_values();
        if !values.is_empty() {
            let value = format!(
                "\nPossible values:\n{}",
                values
                    .into_iter()
                    .filter(|value| !value.is_hide_set())
                    .map(|value| {
                        let name = value.get_name();
                        value.get_help().map_or_else(
                            || format!(" - `{name}`"),
                            |help| format!(" - `{name}`:  {help}"),
                        )
                    })
                    .collect_vec()
                    .join("\n"),
            );
            output.push_str(&markdown::to_html(&value));
        }
    }

    #[test]
    fn generate_cli_reference() -> Result<()> {
        let mode = if EnvVars::is_set(EnvVars::PREK_GENERATE) {
            Mode::Write
        } else {
            Mode::Check
        };

        let reference_string = generate(Cli::command());
        let filename = "cli.md";
        let reference_path = PathBuf::from(ROOT_DIR).join("docs").join(filename);

        match mode {
            Mode::DryRun => {
                anstream::println!("{reference_string}");
            }
            Mode::Check => match fs_err::read_to_string(reference_path) {
                Ok(current) => {
                    if current == reference_string {
                        anstream::println!("Up-to-date: {filename}");
                    } else {
                        let comparison = StrComparison::new(&current, &reference_string);
                        bail!("{filename} changed, please run `mise run generate`:\n{comparison}");
                    }
                }
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                    bail!("{filename} not found, please run `mise run generate`");
                }
                Err(err) => {
                    bail!("{filename} changed, please run `mise run generate`:\n{err}");
                }
            },
            Mode::Write => match fs_err::read_to_string(&reference_path) {
                Ok(current) => {
                    if current == reference_string {
                        anstream::println!("Up-to-date: {filename}");
                    } else {
                        anstream::println!("Updating: {filename}");
                        fs_err::write(reference_path, reference_string.as_bytes())?;
                    }
                }
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                    anstream::println!("Updating: {filename}");
                    fs_err::write(reference_path, reference_string.as_bytes())?;
                }
                Err(err) => {
                    bail!(
                        "{filename} changed, please run `cargo dev generate-cli-reference`:\n{err}"
                    );
                }
            },
        }

        Ok(())
    }
}
