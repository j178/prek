use std::ffi::OsString;
use std::fmt::Write;
use std::io::Read;
use std::ops::RangeInclusive;
use std::path::PathBuf;

use anstream::eprintln;
use anyhow::Result;
use itertools::Itertools;
use owo_colors::OwoColorize;

use prek_consts::env_vars::EnvVars;

use crate::cli::{self, ExitStatus, RunArgs};
use crate::config::HookType;
use crate::fs::CWD;
use crate::git::GIT_ROOT;
use crate::printer::Printer;
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
    _hook_dir: PathBuf,
    skip_on_missing_config: bool,
    script_version: Option<usize>,
    args: Vec<OsString>,
    printer: Printer,
) -> Result<ExitStatus> {
    // TODO: run in legacy mode

    if script_version != Some(cli::install::CUR_SCRIPT_VERSION) {
        warn_user!(
            "The installed hook script `{hook_type}` is outdated (version: {:?}, expected: {}). Please reinstall the hooks with `prek install`.",
            script_version.unwrap_or(1),
            cli::install::CUR_SCRIPT_VERSION
        );
    }

    let allow_missing_config =
        skip_on_missing_config || EnvVars::is_set(EnvVars::PREK_ALLOW_NO_CONFIG);
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
                Ok(ExitStatus::Success)
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
                    Ok(ExitStatus::Success)
                } else {
                    eprintln!("{}: {e}", "error".red().bold());
                    warn_for_no_config();

                    Ok(ExitStatus::Failure)
                };
            }
            Ok(project) => {
                if project.path() != GIT_ROOT.as_ref()? {
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

    if !hook_type.num_args().contains(&args.len()) {
        anyhow::bail!(
            "hook `{}` expects {} but received {}{}",
            hook_type.to_string().cyan(),
            format_expected_args(hook_type.num_args()),
            format_received_args(args.len()),
            format_argument_dump(&args)
        );
    }

    let Some(run_args) = to_run_args(hook_type, &args).await else {
        return Ok(ExitStatus::Success);
    };

    cli::run(
        store,
        config,
        includes,
        skips,
        Some(hook_type.into()),
        run_args.from_ref,
        run_args.to_ref,
        run_args.all_files,
        vec![],
        vec![],
        false,
        false,
        run_args.fail_fast,
        false,
        false,
        run_args.extra,
        false,
        printer,
    )
    .await
}

async fn to_run_args(hook_type: HookType, args: &[OsString]) -> Option<RunArgs> {
    let mut run_args = RunArgs::default();

    match hook_type {
        HookType::PrePush => {
            // https://git-scm.com/docs/githooks#_pre_push
            run_args.extra.remote_name = Some(args[0].to_string_lossy().into_owned());
            run_args.extra.remote_url = Some(args[1].to_string_lossy().into_owned());

            if let Some(push_info) = parse_pre_push_info(&args[0].to_string_lossy()).await {
                run_args.from_ref = push_info.from_ref;
                run_args.to_ref = push_info.to_ref;
                run_args.all_files = push_info.all_files;
                run_args.extra.remote_branch = push_info.remote_branch;
                run_args.extra.local_branch = push_info.local_branch;
            } else {
                // Nothing to push
                return None;
            }
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
            run_args.from_ref = Some(args[0].to_string_lossy().into_owned());
            run_args.to_ref = Some(args[1].to_string_lossy().into_owned());
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

    Some(run_args)
}

#[derive(Debug)]
struct PushInfo {
    from_ref: Option<String>,
    to_ref: Option<String>,
    all_files: bool,
    remote_branch: Option<String>,
    local_branch: Option<String>,
}

async fn parse_pre_push_info(remote_name: &str) -> Option<PushInfo> {
    // Read from stdin
    let mut stdin = std::io::stdin();
    let mut buffer = String::new();

    if stdin.read_to_string(&mut buffer).is_err() {
        return None;
    }

    for line in buffer.lines() {
        let parts: Vec<&str> = line.rsplitn(4, ' ').collect();
        if parts.len() != 4 {
            continue;
        }

        let local_branch = parts[3];
        let local_sha = parts[2];
        let remote_branch = parts[1];
        let remote_sha = parts[0];

        // Skip if local_sha is all zeros
        if local_sha.bytes().all(|b| b == b'0') {
            continue;
        }

        // If remote_sha exists and is not all zeros
        if !remote_sha.bytes().all(|b| b == b'0')
            && git::rev_exists(remote_sha).await.unwrap_or(false)
        {
            return Some(PushInfo {
                from_ref: Some(remote_sha.to_string()),
                to_ref: Some(local_sha.to_string()),
                all_files: false,
                remote_branch: Some(remote_branch.to_string()),
                local_branch: Some(local_branch.to_string()),
            });
        }

        // Find ancestors that don't exist in remote
        let ancestors = git::get_ancestors_not_in_remote(local_sha, remote_name)
            .await
            .unwrap_or_default();
        if ancestors.is_empty() {
            continue;
        }

        let first_ancestor = &ancestors[0];
        let roots = git::get_root_commits(local_sha).await.unwrap_or_default();

        if roots.contains(first_ancestor) {
            // Pushing the whole tree including root commit
            return Some(PushInfo {
                from_ref: None,
                to_ref: Some(local_sha.to_string()),
                all_files: true,
                remote_branch: Some(remote_branch.to_string()),
                local_branch: Some(local_branch.to_string()),
            });
        }
        // Find the source (first_ancestor^)
        if let Ok(Some(source)) = git::get_parent_commit(first_ancestor).await {
            return Some(PushInfo {
                from_ref: Some(source),
                to_ref: Some(local_sha.to_string()),
                all_files: false,
                remote_branch: Some(remote_branch.to_string()),
                local_branch: Some(local_branch.to_string()),
            });
        }
    }

    // Nothing to push
    None
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
