use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::builder::Styles;
use clap::builder::styling::{AnsiColor, Effects};
use clap::{ArgAction, Args, Parser, Subcommand, ValueHint};

use constants::env_vars::EnvVars;

use crate::config::{CONFIG_FILE, HookType, Stage};

mod clean;
mod hook_impl;
mod install;
mod reporter;
pub mod run;
mod sample_config;
mod self_update;
mod validate;

pub(crate) use clean::clean;
pub(crate) use hook_impl::hook_impl;
pub(crate) use install::{init_template_dir, install, install_hooks, uninstall};
pub(crate) use run::run;
pub(crate) use sample_config::sample_config;
pub(crate) use self_update::self_update;
pub(crate) use validate::{validate_configs, validate_manifest};

#[derive(Copy, Clone)]
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
    name = "prefligit",
    author,
    long_version = crate::version::version(),
    about = "pre-commit reimplemented in Rust"
)]
#[command(propagate_version = true)]
#[command(
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

#[derive(Debug, Parser)]
#[command(next_help_heading = "Global options", next_display_order = 1000)]
#[command(disable_help_flag = true, disable_version_flag = true)]
pub(crate) struct GlobalArgs {
    /// Path to alternate config file.
    #[arg(global = true, short, long, value_parser)]
    pub(crate) config: Option<PathBuf>,

    /// Whether to use color in output.
    #[arg(
        global = true,
        long,
        value_enum,
        env = EnvVars::PREFLIGIT_COLOR,
        default_value_t = ColorChoice::Auto,
    )]
    pub(crate) color: ColorChoice,

    /// Display the concise help for this command.
    #[arg(global = true, short, long, action = clap::ArgAction::HelpShort)]
    help: Option<bool>,

    /// Hide all progress outputs.
    ///
    /// For example, spinners or progress bars.
    #[arg(global = true, long)]
    pub no_progress: bool,

    /// Do not print any output.
    #[arg(global = true, long, short, conflicts_with = "verbose")]
    pub quiet: bool,

    /// Use verbose output.
    #[arg(global = true, short, long, action = ArgAction::Count)]
    pub(crate) verbose: u8,

    /// Display the prefligit version.
    #[arg(global = true, short = 'V', long, action = clap::ArgAction::Version)]
    version: Option<bool>,

    /// Show the resolved settings for the current command.
    ///
    /// This option is used for debugging and development purposes.
    #[arg(global = true, long, hide = true)]
    pub show_settings: bool,
}

#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    /// Install the prefligit git hook.
    Install(InstallArgs),
    /// Create hook environments for all hooks used in the config file.
    ///
    /// This command does not install the git hook. To install the git hook along with the hook environments in one command, use `prefligit install --install-hooks`.
    InstallHooks,
    /// Run hooks.
    Run(Box<RunArgs>),
    /// Uninstall the prefligit git hook.
    Uninstall(UninstallArgs),
    /// Validate `.pre-commit-config.yaml` files.
    ValidateConfig(ValidateConfigArgs),
    /// Validate `.pre-commit-hooks.yaml` files.
    ValidateManifest(ValidateManifestArgs),
    /// Produce a sample `.pre-commit-config.yaml` file.
    SampleConfig(SampleConfigArgs),
    /// Auto-update pre-commit config to the latest repos' versions.
    #[command(name = "auto-update", alias = "autoupdate")]
    AutoUpdate(AutoUpdateArgs),
    /// Clean unused cached repos.
    GC,
    /// Clean out pre-commit files.
    Clean,
    /// Install hook script in a directory intended for use with `git config init.templateDir`.
    #[command(name = "init-template-dir", alias = "init-templatedir")]
    InitTemplateDir(InitTemplateDirArgs),
    /// Try the pre-commit hooks in the current repo.
    TryRepo(Box<RunArgs>),

    /// The implementation of the `pre-commit` hook.
    #[command(hide = true)]
    HookImpl(HookImplArgs),

    /// `prefligit` self management.
    #[command(name = "self")]
    Self_(SelfNamespace),

    /// Generate shell completion scripts.
    #[command(hide = true)]
    GenerateShellCompletion(GenerateShellCompletionArgs),
}

#[derive(Debug, Args)]
pub(crate) struct InstallArgs {
    /// Overwrite existing hooks.
    #[arg(short = 'f', long)]
    pub(crate) overwrite: bool,

    /// Create hook environments for all hooks used in the config file.
    #[arg(long)]
    pub(crate) install_hooks: bool,

    #[arg(short = 't', long = "hook-type", value_name = "HOOK_TYPE", value_enum)]
    pub(crate) hook_types: Vec<HookType>,

    /// Allow a missing `pre-commit` configuration file.
    #[arg(long)]
    pub(crate) allow_missing_config: bool,
}

#[derive(Debug, Args)]
pub(crate) struct UninstallArgs {
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

#[derive(Debug, Clone, Default, Args)]
pub(crate) struct RunArgs {
    /// The hook ID to run.
    #[arg(value_name = "HOOK", value_hint = ValueHint::Other)]
    pub(crate) hook_id: Option<String>,
    /// Run on all files in the repo.
    #[arg(short, long, conflicts_with_all = ["files", "from_ref", "to_ref"])]
    pub(crate) all_files: bool,
    /// Specific filenames to run hooks on.
    #[arg(long, conflicts_with_all = ["all_files", "from_ref", "to_ref"], value_hint = ValueHint::AnyPath)]
    pub(crate) files: Vec<String>,
    /// Run hooks on all files in the specified directories.
    ///
    /// You can specify multiple directories. It can be used in conjunction with `--files`.
    #[arg(short, long, value_name = "DIR", conflicts_with_all = ["all_files", "from_ref", "to_ref"], value_hint = ValueHint::DirPath)]
    pub(crate) directory: Vec<String>,
    /// The original ref in a `from_ref...to_ref` diff expression.
    /// Files changed in this diff will be run through the hooks.
    #[arg(short = 's', long, alias = "source", requires = "to_ref", value_hint = ValueHint::Other)]
    pub(crate) from_ref: Option<String>,
    /// The destination ref in a `from_ref...to_ref` diff expression.
    /// Files changed in this diff will be run through the hooks.
    #[arg(short = 'o', long, alias = "origin", requires = "from_ref", value_hint = ValueHint::Other)]
    pub(crate) to_ref: Option<String>,
    /// Run hooks against the last commit (HEAD~1..HEAD).
    #[arg(long, conflicts_with_all = ["all_files", "files", "directory", "from_ref", "to_ref"])]
    pub(crate) last_commit: bool,
    /// The stage during which the hook is fired.
    #[arg(long, default_value_t = Stage::PreCommit, value_enum)]
    pub(crate) hook_stage: Stage,
    /// When hooks fail, run `git diff` directly afterward.
    #[arg(long)]
    pub(crate) show_diff_on_failure: bool,

    #[command(flatten)]
    pub(crate) extra: RunExtraArgs,
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
        default_missing_value = CONFIG_FILE,
    )]
    pub(crate) file: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub(crate) struct AutoUpdateArgs {
    #[arg(long, default_value_t = true)]
    pub(crate) bleeding_edge: bool,
    #[arg(long)]
    pub(crate) freeze: bool,
    #[arg(long)]
    pub(crate) repo: Option<String>,
    #[arg(short, long, default_value_t = 1)]
    pub(crate) jobs: usize,
}

#[derive(Debug, Args)]
pub(crate) struct HookImplArgs {
    #[arg(long)]
    pub(crate) hook_type: HookType,
    #[arg(long)]
    pub(crate) hook_dir: PathBuf,
    #[arg(long)]
    pub(crate) skip_on_missing_config: bool,
    #[arg(last = true)]
    pub(crate) args: Vec<OsString>,
}

#[derive(Debug, Args)]
pub struct SelfNamespace {
    #[command(subcommand)]
    pub command: SelfCommand,
}

#[derive(Debug, Subcommand)]
pub enum SelfCommand {
    /// Update prefligit.
    Update(SelfUpdateArgs),
}

#[derive(Debug, Args)]
pub struct SelfUpdateArgs {
    /// Update to the specified version.
    /// If not provided, prefligit will update to the latest version.
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

    /// Which hook type to install.
    #[arg(short = 't', long = "hook-type", value_name = "HOOK_TYPE", value_enum)]
    pub(crate) hook_types: Vec<HookType>,
}
