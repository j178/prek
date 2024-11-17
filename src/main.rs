use std::fmt::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::str::FromStr;

use anstream::{eprintln, ColorChoice};
use anyhow::{Context, Result};
use clap::{CommandFactory, Parser};
use owo_colors::OwoColorize;
use tracing::{debug, error};
use tracing_subscriber::filter::Directive;
use tracing_subscriber::EnvFilter;

use crate::cli::{Cli, Command, ExitStatus, SelfCommand, SelfNamespace, SelfUpdateArgs};
use crate::git::get_root;
use crate::printer::Printer;

mod cli;
mod config;
mod fs;
mod git;
mod hook;
mod identify;
mod languages;
mod printer;
#[cfg(all(unix, feature = "profiler"))]
mod profiler;
mod run;
mod store;
mod warnings;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Level {
    /// Suppress all tracing output by default (overridable by `RUST_LOG`).
    #[default]
    Default,
    /// Show debug messages by default (overridable by `RUST_LOG`).
    Verbose,
    /// Show messages in a hierarchical span tree. By default, debug messages are shown (overridable by `RUST_LOG`).
    ExtraVerbose,
}

fn setup_logging(level: Level) -> Result<()> {
    let directive = match level {
        Level::Default => tracing::level_filters::LevelFilter::OFF.into(),
        Level::Verbose => Directive::from_str("pre_commit=debug")?,
        Level::ExtraVerbose => Directive::from_str("pre_commit=trace")?,
    };

    let filter = EnvFilter::builder()
        .with_default_directive(directive)
        .from_env()
        .context("Invalid RUST_LOG directive")?;

    let ansi = match anstream::Stderr::choice(&std::io::stderr()) {
        ColorChoice::Always | ColorChoice::AlwaysAnsi => true,
        ColorChoice::Never => false,
        // We just asked anstream for a choice, that can't be auto
        ColorChoice::Auto => unreachable!(),
    };

    let format = tracing_subscriber::fmt::format()
        .with_target(false)
        .without_time()
        .with_ansi(ansi);
    tracing_subscriber::fmt::fmt()
        .with_env_filter(filter)
        .event_format(format)
        .with_writer(std::io::stderr)
        .init();
    Ok(())
}

/// Adjusts relative paths in the CLI arguments to be relative to the new working directory.
fn adjust_relative_paths(cli: &mut Cli, new_cwd: &Path) -> Result<()> {
    if let Some(path) = &mut cli.globals.config {
        if path.exists() {
            *path = std::path::absolute(&*path)?;
        }
    }

    if let Some(Command::Run(ref mut args) | Command::TryRepo(ref mut args)) = cli.command {
        args.files = args
            .files
            .iter()
            .map(|path| fs::relative_to(std::path::absolute(path)?, new_cwd))
            .collect::<Result<Vec<PathBuf>, std::io::Error>>()?;
        args.extra.commit_msg_filename = args
            .extra
            .commit_msg_filename
            .as_ref()
            .map(|path| fs::relative_to(std::path::absolute(path)?, new_cwd))
            .transpose()?;
    }

    Ok(())
}

async fn run(mut cli: Cli) -> Result<ExitStatus> {
    ColorChoice::write_global(cli.globals.color.into());

    setup_logging(match cli.globals.verbose {
        0 => Level::Default,
        1 => Level::Verbose,
        _ => Level::ExtraVerbose,
    })?;

    let printer = if cli.globals.quiet {
        Printer::Quiet
    } else if cli.globals.verbose > 0 {
        Printer::Verbose
    } else if cli.globals.no_progress {
        Printer::NoProgress
    } else {
        Printer::Default
    };

    if cli.globals.quiet {
        warnings::disable();
    } else {
        warnings::enable();
    }

    if cli.command.is_none() {
        cli.command = Some(Command::Run(Box::new(cli.run_args.clone())));
    }

    debug!("pre-commit: {}", env!("CARGO_PKG_VERSION"));

    match get_root().await {
        Ok(root) => {
            debug!("Git root: {}", root.display());

            // Adjust relative paths before changing the working directory.
            adjust_relative_paths(&mut cli, &root)?;

            std::env::set_current_dir(&root)?;
        }
        Err(err) => {
            error!("Failed to find git root: {}", err);
        }
    }

    // TODO: read git commit info

    macro_rules! show_settings {
        ($arg:expr) => {
            if cli.globals.show_settings {
                writeln!(printer.stdout(), "{:#?}", $arg)?;
                return Ok(ExitStatus::Success);
            }
        };
        ($arg:expr, false) => {
            if cli.globals.show_settings {
                writeln!(printer.stdout(), "{:#?}", $arg)?;
            }
        };
    }
    show_settings!(cli.globals, false);

    match cli.command.unwrap() {
        Command::Install(args) => {
            show_settings!(args);

            cli::install(
                cli.globals.config,
                args.hook_types,
                args.install_hooks,
                args.overwrite,
                args.allow_missing_config,
                printer,
            )
            .await
        }
        Command::Uninstall(args) => {
            show_settings!(args);

            cli::uninstall(cli.globals.config, args.hook_types, printer).await
        }
        Command::Run(args) => {
            show_settings!(args);

            cli::run(
                cli.globals.config,
                args.hook_id,
                args.hook_stage,
                args.from_ref,
                args.to_ref,
                args.all_files,
                args.files,
                args.show_diff_on_failure,
                args.extra,
                cli.globals.verbose > 0,
                printer,
            )
            .await
        }
        Command::HookImpl(args) => {
            show_settings!(args);

            cli::hook_impl(
                cli.globals.config,
                args.hook_type,
                args.hook_dir,
                args.skip_on_missing_config,
                args.args,
                printer,
            )
            .await
        }
        Command::Clean => cli::clean(printer),
        Command::ValidateConfig(args) => {
            show_settings!(args);

            Ok(cli::validate_configs(args.configs))
        }
        Command::ValidateManifest(args) => {
            show_settings!(args);

            Ok(cli::validate_manifest(args.manifests))
        }
        Command::SampleConfig => Ok(cli::sample_config()),
        Command::Self_(SelfNamespace {
            command:
                SelfCommand::Update(SelfUpdateArgs {
                    target_version,
                    token,
                }),
        }) => cli::self_update(target_version, token, printer).await,
        Command::GenerateShellCompletion(args) => {
            show_settings!(args);

            let mut command = Cli::command();
            let bin_name = command
                .get_bin_name()
                .unwrap_or_else(|| command.get_name())
                .to_owned();
            clap_complete::generate(args.shell, &mut command, bin_name, &mut std::io::stdout());
            Ok(ExitStatus::Success)
        }
        _ => {
            writeln!(printer.stderr(), "Command not implemented yet")?;
            Ok(ExitStatus::Failure)
        }
    }
}

fn main() -> ExitCode {
    ctrlc::set_handler(move || {
        #[allow(clippy::exit, clippy::cast_possible_wrap)]
        std::process::exit(if cfg!(windows) {
            0xC000_013A_u32 as i32
        } else {
            130
        });
    })
    .expect("Error setting Ctrl-C handler");

    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(err) => err.exit(),
    };

    // Initialize the profiler guard if the feature is enabled.
    let mut _profiler_guard = None;
    #[cfg(all(unix, feature = "profiler"))]
    {
        _profiler_guard = profiler::start_profiling();
    }
    #[cfg(not(all(unix, feature = "profiler")))]
    {
        _profiler_guard = Some(());
    }

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("Failed to create tokio runtime");
    let result = runtime.block_on(Box::pin(run(cli)));
    runtime.shutdown_background();

    // Report the profiler if the feature is enabled
    #[cfg(all(unix, feature = "profiler"))]
    {
        profiler::finish_profiling(_profiler_guard);
    }

    match result {
        Ok(code) => code.into(),
        Err(err) => {
            let mut causes = err.chain();
            eprintln!("{}: {}", "error".red().bold(), causes.next().unwrap());
            for err in causes {
                eprintln!("  {}: {}", "caused by".red().bold(), err);
            }
            ExitStatus::Error.into()
        }
    }
}
