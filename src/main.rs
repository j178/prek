use std::fmt::Write;
use std::path::Path;
use std::process::ExitCode;
use std::str::FromStr;
use std::sync::Mutex;

use anstream::{ColorChoice, StripStream, eprintln};
use anyhow::{Context, Result};
use clap::{CommandFactory, Parser};
use clap_complete::CompleteEnv;
use owo_colors::OwoColorize;
use tracing::level_filters::LevelFilter;
use tracing::{debug, error};
use tracing_subscriber::filter::Directive;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};

use crate::cleanup::cleanup;
use crate::cli::{Cli, Command, ExitStatus};
#[cfg(feature = "self-update")]
use crate::cli::{SelfCommand, SelfNamespace, SelfUpdateArgs};
use crate::git::GIT_ROOT;
use crate::printer::Printer;
use crate::run::USE_COLOR;
use crate::store::STORE;

mod archive;
mod builtin;
mod cleanup;
mod cli;
mod config;
mod fs;
mod git;
mod hook;
mod identify;
mod languages;
mod printer;
mod process;
#[cfg(all(unix, feature = "profiler"))]
mod profiler;
mod run;
mod store;
mod version;
mod warnings;
mod workspace;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Level {
    /// Suppress all tracing output by default (overridable by `RUST_LOG`).
    #[default]
    Default,
    /// Show verbose messages.
    Verbose,
    /// Show debug messages by default (overridable by `RUST_LOG`).
    Debug,
    /// Show trace messages by default (overridable by `RUST_LOG`).
    Trace,
    /// Show trace messages for all crates by default (overridable by `RUST_LOG`).
    TraceAll,
}

fn setup_logging(level: Level) -> Result<()> {
    let directive = match level {
        Level::Default | Level::Verbose => LevelFilter::OFF.into(),
        Level::Debug => Directive::from_str("prek=debug")?,
        Level::Trace => Directive::from_str("prek=trace")?,
        Level::TraceAll => Directive::from_str("trace")?,
    };

    let stderr_filter = EnvFilter::builder()
        .with_default_directive(directive)
        .from_env()
        .context("Invalid RUST_LOG directive")?;
    let stderr_format = tracing_subscriber::fmt::format()
        .with_target(false)
        .without_time()
        .with_ansi(*USE_COLOR);
    let stderr_layer = tracing_subscriber::fmt::layer()
        .event_format(stderr_format)
        .with_writer(anstream::stderr)
        .with_filter(stderr_filter);

    let log_file_path = STORE.as_ref()?.log_file();
    let log_file = fs_err::OpenOptions::new()
        .create(true)
        .write(true)
        .open(log_file_path)
        .context("Failed to open log file")?;
    let log_file = Mutex::new(StripStream::new(log_file.into_file()));

    let file_format = tracing_subscriber::fmt::format()
        .with_target(false)
        .with_ansi(false);
    let file_layer = tracing_subscriber::fmt::layer()
        .event_format(file_format)
        .with_writer(log_file)
        .with_filter(EnvFilter::new("prek=trace"));

    tracing_subscriber::registry()
        .with(stderr_layer)
        .with(file_layer)
        .init();

    Ok(())
}

fn should_change_cwd(cli: &Cli) -> bool {
    cli.command.as_ref().is_some_and(|cmd| {
        !matches!(
            cmd,
            Command::Clean
                | Command::GC
                | Command::InitTemplateDir(_)
                | Command::SampleConfig(_)
                | Command::ValidateConfig(_)
                | Command::ValidateManifest(_)
        )
    })
}

/// Adjusts relative paths in the CLI arguments to be relative to the new working directory.
fn adjust_relative_paths(cli: &mut Cli, new_cwd: &Path) -> Result<()> {
    if let Some(path) = &mut cli.globals.config {
        if path.exists() {
            *path = std::path::absolute(&*path)?;
        }
    }

    // Adjust path arguments for `run` and `try-repo` commands.
    if let Some(Command::Run(ref mut args) | Command::TryRepo(ref mut args)) = cli.command {
        args.files = args
            .files
            .iter()
            .map(|path| {
                fs::relative_to(std::path::absolute(path)?, new_cwd)
                    .map(|p| p.to_string_lossy().to_string())
            })
            .collect::<Result<Vec<String>, std::io::Error>>()?;
        args.directory = args
            .directory
            .iter()
            .map(|path| {
                fs::relative_to(std::path::absolute(path)?, new_cwd)
                    .map(|p| p.to_string_lossy().to_string())
            })
            .collect::<Result<Vec<String>, std::io::Error>>()?;
        args.extra.commit_msg_filename = args
            .extra
            .commit_msg_filename
            .as_ref()
            .map(|path| {
                fs::relative_to(std::path::absolute(path)?, new_cwd)
                    .map(|p| p.to_string_lossy().to_string())
            })
            .transpose()?;
    }

    Ok(())
}

async fn run(mut cli: Cli) -> Result<ExitStatus> {
    ColorChoice::write_global(cli.globals.color.into());

    setup_logging(match cli.globals.verbose {
        0 => Level::Default,
        1 => Level::Verbose,
        2 => Level::Debug,
        3 => Level::Trace,
        _ => Level::TraceAll,
    })?;

    let printer = if cli.globals.quiet {
        Printer::Quiet
    } else if cli.globals.verbose > 1 {
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

    debug!("prek: {}", version::version());

    // Adjust relative paths before changing the working directory.
    if should_change_cwd(&cli) {
        match GIT_ROOT.as_ref() {
            Ok(root) => {
                debug!("Git root: {}", root.display());
                adjust_relative_paths(&mut cli, root)?;

                std::env::set_current_dir(root)?;
            }
            Err(err) => {
                error!("Failed to find git root: {}", err);
            }
        }
    }

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
                None,
            )
            .await
        }
        Command::InstallHooks => cli::install_hooks(cli.globals.config, printer).await,
        Command::Uninstall(args) => {
            show_settings!(args);

            cli::uninstall(cli.globals.config, args.hook_types, printer).await
        }
        Command::Run(args) => {
            show_settings!(args);

            cli::run(
                cli.globals.config,
                args.hook_ids,
                args.hook_stage,
                args.from_ref,
                args.to_ref,
                args.all_files,
                args.files,
                args.directory,
                args.last_commit,
                args.show_diff_on_failure,
                args.extra,
                cli.globals.verbose > 0,
                printer,
            )
            .await
        }
        Command::List(args) => {
            show_settings!(args);

            cli::list(
                cli.globals.config,
                cli.globals.verbose > 0,
                args.hook_ids,
                args.hook_stage,
                args.language,
                args.output_format,
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
        Command::SampleConfig(args) => cli::sample_config(args.file, printer),
        Command::AutoUpdate(args) => {
            cli::auto_update(
                cli.globals.config,
                args.repo,
                args.bleeding_edge,
                args.freeze,
                args.jobs,
                printer,
            )
            .await
        }
        #[cfg(feature = "self-update")]
        Command::Self_(SelfNamespace {
            command:
                SelfCommand::Update(SelfUpdateArgs {
                    target_version,
                    token,
                }),
        }) => cli::self_update(target_version, token, printer).await,
        #[cfg(not(feature = "self-update"))]
        Command::Self_(_) => {
            anyhow::bail!(
                "prek was installed through an external package manager, and self-update \
                is not available. Please use your package manager to update prek."
            );
        }

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
        Command::InitTemplateDir(args) => {
            show_settings!(args);

            cli::init_template_dir(
                args.directory,
                cli.globals.config,
                args.hook_types,
                args.no_allow_missing_config,
                printer,
            )
            .await
        }
        _ => {
            writeln!(printer.stderr(), "Command not implemented yet")?;
            Ok(ExitStatus::Failure)
        }
    }
}

fn main() -> ExitCode {
    CompleteEnv::with_factory(Cli::command).complete();

    ctrlc::set_handler(move || {
        cleanup();

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
