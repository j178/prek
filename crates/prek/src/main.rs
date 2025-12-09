use std::fmt::Write;
use std::path::PathBuf;
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
use crate::store::Store;

mod archive;
mod cleanup;
mod cli;
mod config;
mod fs;
mod git;
mod hook;
mod hooks;
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
mod yaml;

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

enum LogFile {
    Default,
    Path(PathBuf),
    Disabled,
}

impl LogFile {
    fn from_args(log_file: Option<PathBuf>, no_log_file: bool) -> Self {
        if no_log_file {
            Self::Disabled
        } else if let Some(path) = log_file {
            Self::Path(path)
        } else {
            Self::Default
        }
    }

    fn is_disabled(&self) -> bool {
        matches!(self, Self::Disabled)
    }
}

fn setup_logging(level: Level, log_file: LogFile, store: &Store) -> Result<()> {
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
        .with_ansi(*USE_COLOR);
    let stderr_layer = tracing_subscriber::fmt::layer()
        .with_span_events(FmtSpan::CLOSE)
        .event_format(stderr_format)
        .with_writer(anstream::stderr)
        .with_filter(stderr_filter);

    let registry = tracing_subscriber::registry().with(stderr_layer);

    if log_file.is_disabled() {
        registry.init();
    } else {
        let log_file_path = match log_file {
            LogFile::Default => store.log_file(),
            LogFile::Path(path) => path,
            LogFile::Disabled => unreachable!(),
        };
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

        registry.with(file_layer).init();
    }

    Ok(())
}

async fn run(mut cli: Cli) -> Result<ExitStatus> {
    // Enabled ANSI colors on Windows.
    let _ = anstyle_query::windows::enable_ansi_colors();

    ColorChoice::write_global(cli.globals.color.into());

    let store = Store::from_settings()?;
    let log_file = LogFile::from_args(cli.globals.log_file.clone(), cli.globals.no_log_file);
    setup_logging(
        match cli.globals.verbose {
            0 => Level::Default,
            1 => Level::Verbose,
            2 => Level::Debug,
            3 => Level::Trace,
            _ => Level::TraceAll,
        },
        log_file,
        &store,
    )?;

    let printer = if cli.globals.quiet == 1 {
        Printer::Quiet
    } else if cli.globals.quiet > 1 {
        Printer::Silent
    } else if cli.globals.verbose > 1 {
        Printer::Verbose
    } else if cli.globals.no_progress {
        Printer::NoProgress
    } else {
        Printer::Default
    };

    if cli.globals.quiet > 0 {
        warnings::disable();
    } else {
        warnings::enable();
    }

    if cli.command.is_none() {
        cli.command = Some(Command::Run(Box::new(cli.run_args.clone())));
    }

    debug!("prek: {}", version::version());

    if let Some(dir) = cli.globals.cd.as_ref() {
        debug!("Changing current directory to: `{}`", dir.display());
        std::env::set_current_dir(dir)?;
    }

    debug!("Args: {:?}", std::env::args().collect::<Vec<_>>());

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
                &store,
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
            cli::install_hooks(
                &store,
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
                &store,
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
                args.fail_fast,
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
                &store,
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
                &store,
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
            CacheCommand::Clean => cli::cache_clean(&store, printer),
            CacheCommand::Dir => {
                writeln!(printer.stdout(), "{}", store.path().display().cyan())?;
                Ok(ExitStatus::Success)
            }
            CacheCommand::GC => {
                writeln!(printer.stderr(), "Command not implemented yet")?;
                Ok(ExitStatus::Failure)
            }
            CacheCommand::Size(cli::SizeArgs { human }) => cli::cache_size(&store, human, printer),
        },
        Command::Clean => cli::cache_clean(&store, printer),
        Command::ValidateConfig(args) => {
            show_settings!(args);

            cli::validate_configs(args.configs, printer)
        }
        Command::ValidateManifest(args) => {
            show_settings!(args);

            cli::validate_manifest(args.manifests, printer)
        }
        Command::SampleConfig(args) => cli::sample_config(args.file, printer),
        Command::AutoUpdate(args) => {
            cli::auto_update(
                &store,
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
        Command::TryRepo(args) => {
            show_settings!(args);

            cli::try_repo(
                cli.globals.config,
                args.repo,
                args.rev,
                args.run_args,
                cli.globals.refresh,
                cli.globals.verbose > 0,
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
                &store,
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

    #[cfg(all(unix, feature = "profiler"))]
    let _profiler_guard = profiler::start_profiling();

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("Failed to create tokio runtime");
    let result = runtime.block_on(Box::pin(run(cli)));
    runtime.shutdown_background();

    // Report the profiler if the feature is enabled
    #[cfg(all(unix, feature = "profiler"))]
    profiler::finish_profiling(_profiler_guard);

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
