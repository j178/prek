use std::ffi::OsString;
use std::io::{self, Read};
use std::path::PathBuf;

use anstream::eprintln;
use anyhow::Result;
use owo_colors::OwoColorize;

use constants::env_vars::EnvVars;

use crate::cli::{self, ExitStatus, RunArgs};
use crate::config::HookType;
use crate::git;
use crate::printer::Printer;

pub(crate) async fn hook_impl(
    config: Option<PathBuf>,
    hook_type: HookType,
    _hook_dir: PathBuf,
    skip_on_missing_config: bool,
    args: Vec<OsString>,
    printer: Printer,
) -> Result<ExitStatus> {
    // TODO: run in legacy mode

    if let Some(ref config_file) = config {
        if !config_file.try_exists()? {
            return if skip_on_missing_config || EnvVars::is_set(EnvVars::PREK_ALLOW_NO_CONFIG) {
                Ok(ExitStatus::Success)
            } else {
                eprintln!(
                    "{}: config file not found: `{}`",
                    "error".red().bold(),
                    config_file.display().cyan()
                );
                eprintln!(
                    "- To temporarily silence this, run `{}`",
                    format!("{}=1 git ...", EnvVars::PREK_ALLOW_NO_CONFIG).cyan()
                );
                eprintln!(
                    "- To permanently silence this, install hooks with the `{}` flag",
                    "--allow-missing-config".cyan()
                );
                eprintln!("- To uninstall hooks, run `{}`", "prek uninstall".cyan());
                Ok(ExitStatus::Failure)
            };
        }
    }

    if !hook_type.num_args().contains(&args.len()) {
        eprintln!("Invalid number of arguments for hook: {}", hook_type);
        return Ok(ExitStatus::Failure);
    }

    let Some(run_args) = to_run_args(hook_type, &args).await else {
        return Ok(ExitStatus::Success);
    };

    cli::run(
        config,
        run_args.hook_ids,
        hook_type.into(),
        run_args.from_ref,
        run_args.to_ref,
        run_args.all_files,
        vec![],
        vec![],
        false, // last_commit is always false in hook implementation context
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
    let mut stdin = io::stdin();
    let mut buffer = String::new();

    if stdin.read_to_string(&mut buffer).is_err() {
        return None;
    }

    let z40 = "0".repeat(40);

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
        if local_sha == z40 {
            continue;
        }

        // If remote_sha exists and is not all zeros, and remote SHA exists
        if remote_sha != z40 && git::rev_exists(remote_sha).await.unwrap_or(false) {
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
