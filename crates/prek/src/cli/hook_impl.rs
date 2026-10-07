use std::ffi::OsString;
use std::fmt::Write;
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use anstream::eprintln;
use anyhow::Result;
use itertools::Itertools;
use owo_colors::OwoColorize;
use prek_consts::env_vars::{EnvVars, EnvVarsRead};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::cli::{self, ExitStatus, RunArgs, RunOptions};
use crate::config::HookType;
use crate::fs::CWD;
use crate::languages::resolve_command;
use crate::printer::Printer;
use crate::process::Cmd;
use crate::store::Store;
use crate::workspace;
use crate::workspace::Project;
use crate::{git, warn_user};

pub(crate) async fn hook_impl(
    store: &Store,
    config: Option<PathBuf>,
    includes: Vec<String>,
    skips: Vec<String>,
    hook_type: HookType,
    hook_dir: Option<PathBuf>,
    skip_on_missing_config: bool,
    script_version: Option<usize>,
    args: Vec<OsString>,
    printer: Printer,
) -> Result<ExitStatus> {
    let stdin = read_hook_stdin(hook_type).await?;
    let legacy_code = run_legacy(hook_type, hook_dir.as_deref(), &args, &stdin).await?;

    if let Some(script_version) = script_version
        && script_version != cli::install::CUR_SCRIPT_VERSION
    {
        warn_user!(
            "The installed Git shim `{hook_type}` is outdated (version: {script_version}, expected: {}). Please reinstall the Git shims with `prek install`.",
            cli::install::CUR_SCRIPT_VERSION
        );
    }

    let allow_missing_config =
        skip_on_missing_config || EnvVars.is_set(EnvVars::PREK_ALLOW_NO_CONFIG);
    let warn_for_no_config = || {
        eprintln!(
            "- To temporarily silence this, run `{}`",
            format!("{}=1 git ...", EnvVars::PREK_ALLOW_NO_CONFIG).cyan()
        );
        eprintln!(
            "- To permanently silence this, install hooks with the `{}` flag",
            "--allow-missing-config".cyan()
        );
        eprintln!("- To uninstall hooks, run `{}`", "prek uninstall".cyan());
    };

    // Check if there is config file
    if let Some(ref config) = config {
        if !config.try_exists()? {
            return if allow_missing_config {
                Ok(legacy_code.into())
            } else {
                eprintln!(
                    "{}: config file not found: `{}`",
                    "error".red().bold(),
                    config.display().cyan()
                );
                warn_for_no_config();

                Ok(ExitStatus::Failure)
            };
        }
        writeln!(printer.stdout(), "Using config file: {}", config.display())?;
    } else {
        // Try to discover a project from current directory (after `--cd`)
        match Project::discover(config.as_deref(), &CWD) {
            Err(e @ workspace::Error::MissingConfigFile) => {
                return if allow_missing_config {
                    Ok(legacy_code.into())
                } else {
                    eprintln!("{}: {e}", "error".red().bold());
                    warn_for_no_config();

                    Ok(ExitStatus::Failure)
                };
            }
            Ok(project) => {
                if project.path() != git::root()? {
                    writeln!(
                        printer.stdout(),
                        "Running in workspace: `{}`",
                        project.path().display().cyan()
                    )?;
                }
            }
            Err(e) => return Err(e.into()),
        }
    }

    let expected_args = hook_num_args(hook_type);
    if !expected_args.contains(&args.len()) {
        anyhow::bail!(
            "hook `{}` expects {} but received {}{}",
            hook_type.to_string().cyan(),
            format_expected_args(expected_args),
            format_received_args(args.len()),
            format_argument_dump(&args)
        );
    }

    let runs = hook_run_options(hook_type, &args, &stdin).await?;
    let multiple_refs = runs.len() > 1;
    for mut run_args in runs {
        if multiple_refs
            && let (Some(local), Some(remote)) =
                (&run_args.extra.local_branch, &run_args.extra.remote_branch)
        {
            writeln!(
                printer.stdout(),
                "Running pre-push hooks for `{}` -> `{}`",
                local.cyan(),
                remote.cyan(),
            )?;
        }
        run_args.includes.clone_from(&includes);
        run_args.skips.clone_from(&skips);

        // Each run may change to the workspace root. Keep relative paths anchored
        // to the original directory for every ref.
        std::env::set_current_dir(&*CWD)?;
        let status = cli::run(
            store,
            config.clone(),
            RunArgs {
                options: run_args,
                stage: Some(hook_type.into()),
                ..RunArgs::default()
            },
            false,
            false,
            printer,
        )
        .await?;

        if !matches!(status, ExitStatus::Success) {
            return Ok(status);
        }
    }

    Ok(legacy_code.into())
}

fn hook_num_args(hook_type: HookType) -> RangeInclusive<usize> {
    match hook_type {
        HookType::CommitMsg => 1..=1,
        HookType::PostCheckout => 3..=3,
        HookType::PreCommit => 0..=0,
        HookType::PostCommit => 0..=0,
        HookType::PreMergeCommit => 0..=0,
        HookType::PostMerge => 1..=1,
        HookType::PostRewrite => 1..=1,
        HookType::PrePush => 2..=2,
        HookType::PreRebase => 1..=2,
        HookType::PrepareCommitMsg => 1..=3,
    }
}

async fn read_hook_stdin(hook_type: HookType) -> Result<Vec<u8>> {
    if !matches!(hook_type, HookType::PrePush) {
        return Ok(vec![]);
    }

    let mut stdin = tokio::io::stdin();
    let mut buffer = vec![];
    stdin.read_to_end(&mut buffer).await?;
    Ok(buffer)
}

async fn run_legacy(
    hook_type: HookType,
    hook_dir: Option<&Path>,
    args: &[OsString],
    stdin: &[u8],
) -> Result<u8> {
    if EnvVars.is_set(EnvVars::PREK_RUNNING_LEGACY) {
        anyhow::bail!(
            "prek's Git shim is installed in migration mode\n\
            run `prek install -f --hook-type {hook_type}` to reinstall the shim"
        );
    }

    // `prek hook-impl` without `--hook-dir` is likely invoked from Git 2.54+
    // config-based hooks, where there is no hook script directory to inspect.
    // Skip legacy hooks in that case.
    let Some(hook_dir) = hook_dir else {
        return Ok(0);
    };
    let legacy_hook = hook_dir.join(format!("{hook_type}.legacy"));
    let metadata = match fs_err::tokio::metadata(&legacy_hook).await {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // No legacy hook, so skip running it.
            return Ok(0);
        }
        Err(e) => return Err(e.into()),
    };
    let executable;
    #[cfg(unix)]
    {
        executable = crate::fs::has_executable_bit(&metadata);
    }
    #[cfg(not(unix))]
    {
        executable = true;
        _ = metadata;
    }
    if !executable {
        return Ok(0);
    }

    let entry = resolve_command(vec![legacy_hook.into_os_string()], None, &CWD);
    let mut cmd = Cmd::new(&entry[0]);
    cmd.check(false).args(&entry[1..]).args(args);
    cmd.env(EnvVars::PREK_RUNNING_LEGACY, "1");

    let status = if stdin.is_empty() {
        cmd.status().await?
    } else {
        cmd.stdin(Stdio::piped());
        let mut child = cmd.spawn()?;
        if let Some(mut child_stdin) = child.stdin.take() {
            child_stdin.write_all(stdin).await?;
        }
        child.wait().await?
    };

    Ok(status
        .code()
        .and_then(|code| u8::try_from(code).ok())
        .unwrap_or(1))
}

async fn hook_run_options(
    hook_type: HookType,
    args: &[OsString],
    stdin: &[u8],
) -> Result<Vec<RunOptions>> {
    let mut run_args = RunOptions::default();

    match hook_type {
        HookType::PrePush => {
            return pre_push_run_options(
                &args[0].to_string_lossy(),
                &args[1].to_string_lossy(),
                stdin,
            )
            .await;
        }
        HookType::CommitMsg => {
            run_args.extra.commit_msg_filename = Some(args[0].to_string_lossy().into_owned());
        }
        HookType::PrepareCommitMsg => {
            run_args.extra.commit_msg_filename = Some(args[0].to_string_lossy().into_owned());
            if args.len() > 1 {
                run_args.extra.prepare_commit_message_source =
                    Some(args[1].to_string_lossy().into_owned());
            }
            if args.len() > 2 {
                run_args.extra.commit_object_name = Some(args[2].to_string_lossy().into_owned());
            }
        }
        HookType::PostCheckout => {
            run_args.file_selection.from_ref = Some(args[0].to_string_lossy().into_owned());
            run_args.file_selection.to_ref = Some(args[1].to_string_lossy().into_owned());
            run_args.extra.checkout_type = Some(args[2].to_string_lossy().into_owned());
        }
        HookType::PostMerge => run_args.extra.is_squash_merge = args[0] == "1",
        HookType::PostRewrite => {
            run_args.extra.rewrite_command = Some(args[0].to_string_lossy().into_owned());
        }
        HookType::PreRebase => {
            run_args.extra.pre_rebase_upstream = Some(args[0].to_string_lossy().into_owned());
            if args.len() > 1 {
                run_args.extra.pre_rebase_branch = Some(args[1].to_string_lossy().into_owned());
            }
        }
        HookType::PostCommit | HookType::PreMergeCommit | HookType::PreCommit => {}
    }

    Ok(vec![run_args])
}

async fn pre_push_run_options(
    remote_name: &str,
    remote_url: &str,
    stdin: &[u8],
) -> Result<Vec<RunOptions>> {
    let buffer = String::from_utf8_lossy(stdin);
    let mut runs = Vec::new();

    // https://git-scm.com/docs/githooks#_pre_push
    for line in buffer.lines() {
        let Some((remote_sha, remote_branch, local_sha, local_branch)) =
            line.rsplitn(4, ' ').collect_tuple()
        else {
            // Ignore malformed lines from stdin; a later valid line may still describe a push.
            continue;
        };

        // A zero local SHA means this push deletes the remote ref. There is no local
        // target commit to diff, so it contributes no files to check.
        if local_sha.bytes().all(|b| b == b'0') {
            continue;
        }

        let from_ref = if !remote_sha.bytes().all(|b| b == b'0')
            && git::rev_exists(remote_sha).await?
            && git::is_ancestor(remote_sha, local_sha).await?
        {
            Some(remote_sha.to_string())
        } else {
            // New refs, missing remote objects, and rebased force-pushes use the
            // parent of the first remote-unknown commit to exclude upstream changes.
            let ancestors = git::ancestors_not_in_remote(local_sha, remote_name).await?;
            let Some(first_ancestor) = ancestors.first() else {
                continue;
            };
            let roots = git::root_commits(local_sha).await?;
            if roots.contains(first_ancestor) {
                // A root push has no diff base and checks the full tracked tree.
                None
            } else if let Some(parent) = git::parent_commit(first_ancestor).await? {
                Some(parent)
            } else {
                continue;
            }
        };

        let mut run_args = RunOptions::default();
        run_args.file_selection.all_files = from_ref.is_none();
        run_args.file_selection.from_ref = from_ref;
        run_args.file_selection.to_ref = Some(local_sha.to_string());
        run_args.extra.remote_branch = Some(remote_branch.to_string());
        run_args.extra.local_branch = Some(local_branch.to_string());
        run_args.extra.remote_name = Some(remote_name.to_string());
        run_args.extra.remote_url = Some(remote_url.to_string());
        runs.push(run_args);
    }

    Ok(runs)
}

fn format_expected_args(range: RangeInclusive<usize>) -> String {
    let (start, end) = (*range.start(), *range.end());
    match (start, end) {
        (0, 0) => "no arguments".to_string(),
        (1, 1) => "exactly 1 argument".to_string(),
        (s, e) if s == e => format!("exactly {s} arguments"),
        (0, e) => format!("up to {e} arguments"),
        (s, usize::MAX) => format!("at least {s} arguments"),
        (s, e) => format!("between {s} and {e} arguments"),
    }
}

fn format_received_args(received: usize) -> String {
    match received {
        0 => "no arguments".to_string(),
        1 => "1 argument".to_string(),
        n => format!("{n} arguments"),
    }
}

fn format_argument_dump(args: &[OsString]) -> String {
    if args.is_empty() {
        String::new()
    } else {
        format!(": `{}`", args.iter().map(|s| s.to_string_lossy()).join(" "))
    }
}
