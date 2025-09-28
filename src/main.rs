use std::fmt::Write;
use std::process::ExitCode;
use std::str::FromStr;
use std::sync::Mutex;

use anstream::{ColorChoice, StripStream, eprintln};
use anyhow::{Context, Result};
use clap::{CommandFactory, Parser};
use clap_complete::CompleteEnv;
use owo_colors::OwoColorize;
use tracing::debug;
use tracing::level_filters::LevelFilter;
use tracing_subscriber::filter::Directive;
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};

use crate::cleanup::cleanup;
use crate::cli::{CacheCommand, CacheNamespace, Cli, Command, ExitStatus};
#[cfg(feature = "self-update")]
use crate::cli::{SelfCommand, SelfNamespace, SelfUpdateArgs};
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
        .with_span_events(FmtSpan::CLOSE)
        .event_format(stderr_format)
        .with_writer(anstream::stderr)
        .with_filter(stderr_filter);

    let log_file_path = STORE.as_ref()?.log_file();
    let log_file = fs_err::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(log_file_path)
        .context("Failed to open log file")?;
    let log_file = Mutex::new(StripStream::new(log_file.into_file()));

    let file_format = tracing_subscriber::fmt::format()
        .with_target(false)
        .with_ansi(false);
    let file_layer = tracing_subscriber::fmt::layer()
        .with_span_events(FmtSpan::CLOSE)
        .event_format(file_format)
        .with_writer(log_file)
        .with_filter(EnvFilter::new("prek=trace"));

    tracing_subscriber::registry()
        .with(stderr_layer)
        .with(file_layer)
        .init();

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

    if let Some(dir) = cli.globals.cd.as_ref() {
        std::env::set_current_dir(dir)?;
    }

    debug!("prek: {}", version::version());

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
                args.includes,
                args.skips,
                args.hook_types,
                args.install_hooks,
                args.overwrite,
                args.allow_missing_config,
                cli.globals.refresh,
                printer,
                None,
            )
            .await
        }
        Command::InstallHooks(args) => {
            // TODO: add selectors?
            cli::install_hooks(
                cli.globals.config,
                args.includes,
                args.skips,
                cli.globals.refresh,
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
                args.includes,
                args.skips,
                args.hook_stage,
                args.from_ref,
                args.to_ref,
                args.all_files,
                args.files,
                args.directory,
                args.last_commit,
                args.show_diff_on_failure,
                args.dry_run,
                cli.globals.refresh,
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
                args.includes,
                args.skips,
                args.hook_stage,
                args.language,
                args.output_format,
                cli.globals.refresh,
                cli.globals.verbose > 0,
                printer,
            )
            .await
        }
        Command::HookImpl(args) => {
            show_settings!(args);

            cli::hook_impl(
                cli.globals.config,
                args.includes,
                args.skips,
                args.hook_type,
                args.hook_dir,
                args.skip_on_missing_config,
                args.script_version,
                args.args,
                printer,
            )
            .await
        }
        Command::Cache(CacheNamespace {
            command: cache_command,
        }) => match cache_command {
            CacheCommand::Clean => cli::clean(printer),
            CacheCommand::Dir => {
                let store = STORE.as_ref()?;
                writeln!(printer.stdout(), "{}", store.path().display().cyan())?;
                Ok(ExitStatus::Success)
            }
            CacheCommand::GC => {
                writeln!(printer.stderr(), "Command not implemented yet")?;
                Ok(ExitStatus::Failure)
            }
        },
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
                args.dry_run,
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
                cli.globals.refresh,
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
