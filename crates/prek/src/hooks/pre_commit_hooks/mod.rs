use std::future::Future;
use std::path::{Path, PathBuf};

use anyhow::Result;
use clap::Parser;
use tracing::debug;

use crate::hook::Hook;
use crate::hooks::{HookOutput, run_concurrent_file_checks};

use super::HookFuture;

pub(crate) mod check_added_large_files;
pub(crate) mod check_case_conflict;
pub(crate) mod check_executables_have_shebangs;
pub(crate) mod check_illegal_windows_names;
pub(crate) mod check_json;
pub(crate) mod check_merge_conflict;
pub(crate) mod check_shebang_scripts_are_executable;
pub(crate) mod check_symlinks;
pub(crate) mod check_toml;
pub(crate) mod check_vcs_permalinks;
pub(crate) mod check_xml;
pub(crate) mod check_yaml;
pub(crate) mod destroyed_symlinks;
pub(crate) mod detect_private_key;
pub(crate) mod file_contents_sorter;
pub(crate) mod fix_byte_order_marker;
pub(crate) mod fix_end_of_file;
pub(crate) mod fix_trailing_whitespace;
pub(crate) mod forbid_new_submodules;
pub(crate) mod mixed_line_ending;
pub(crate) mod no_commit_to_branch;
pub(crate) mod pretty_format_json;
pub(crate) mod requirements_txt_fixer;
pub(crate) mod shebangs;

#[derive(Parser)]
#[command(disable_help_subcommand = true)]
#[command(disable_version_flag = true)]
#[command(disable_help_flag = true)]
pub(crate) struct FilenamesArgs {
    #[arg(value_name = "FILENAMES")]
    pub(crate) filenames: Vec<PathBuf>,
}

pub(crate) fn parse_hook_args<T: Parser>(hook: &Hook) -> Result<T> {
    Ok(T::try_parse_from(
        hook.entry.expect_direct().split_with_args(&hook.args)?,
    )?)
}

pub(crate) fn hook_filenames<'a>(
    configured: &'a [PathBuf],
    selected: &'a [&Path],
) -> impl Iterator<Item = &'a Path> + 'a {
    configured
        .iter()
        .map(PathBuf::as_path)
        .chain(selected.iter().copied())
}

/// Runs explicit filenames serially, followed by selected filenames concurrently.
///
/// Duplicates and overlaps between the two inputs are intentionally preserved.
pub(crate) async fn run_file_checks<'a, F, Fut>(
    explicit: &'a [PathBuf],
    selected: &'a [&Path],
    concurrency: usize,
    check: F,
) -> Result<HookOutput>
where
    F: Fn(&'a Path) -> Fut,
    Fut: Future<Output = Result<HookOutput>>,
{
    // Keep the common case on the concurrent path without an extra accumulator.
    if explicit.is_empty() {
        return run_concurrent_file_checks(selected.iter().copied(), concurrency, check).await;
    }

    // Filenames from `entry` or `args` may repeat or overlap with `selected`, so finish
    // them serially in CLI order before starting the selected batch.
    let mut result = HookOutput::unchanged(0, Vec::new());
    for filename in explicit {
        result.merge_known(check(filename).await?);
    }
    if selected.is_empty() {
        return Ok(result);
    }

    // The explicit batch is complete, so selected filenames retain their normal concurrency.
    let selected_result =
        run_concurrent_file_checks(selected.iter().copied(), concurrency, check).await?;
    result.merge_known(selected_result);
    Ok(result)
}

/// Hooks from `https://github.com/pre-commit/pre-commit-hooks`.
#[derive(strum::EnumString)]
#[strum(serialize_all = "kebab-case")]
pub(crate) enum PreCommitHooks {
    CheckAddedLargeFiles,
    CheckCaseConflict,
    CheckExecutablesHaveShebangs,
    CheckIllegalWindowsNames,
    CheckShebangScriptsAreExecutable,
    CheckVcsPermalinks,
    FileContentsSorter,
    EndOfFileFixer,
    FixByteOrderMarker,
    ForbidNewSubmodules,
    CheckJson,
    CheckSymlinks,
    CheckMergeConflict,
    CheckToml,
    CheckXml,
    CheckYaml,
    DestroyedSymlinks,
    MixedLineEnding,
    DetectPrivateKey,
    NoCommitToBranch,
    RequirementsTxtFixer,
    // `pretty-format-json` is intentionally builtin-only for now. Do not enable
    // automatic fast-path replacement until parity coverage against upstream
    // Python is broad enough to trust it as the default implementation.
    // PrettyFormatJson,
    TrailingWhitespace,
}

impl PreCommitHooks {
    pub(crate) async fn run(self, hook: &Hook, filenames: &[&Path]) -> Result<HookOutput> {
        debug!("Running hook `{}` in fast path", hook.id);
        let future: HookFuture<'_> = match self {
            Self::CheckAddedLargeFiles => Box::pin(check_added_large_files::run(hook, filenames)),
            Self::CheckCaseConflict => Box::pin(check_case_conflict::run(hook, filenames)),
            Self::CheckExecutablesHaveShebangs => {
                Box::pin(check_executables_have_shebangs::run(hook, filenames))
            }
            Self::CheckIllegalWindowsNames => {
                Box::pin(check_illegal_windows_names::run(hook, filenames))
            }
            Self::CheckShebangScriptsAreExecutable => {
                Box::pin(check_shebang_scripts_are_executable::run(hook, filenames))
            }
            Self::CheckVcsPermalinks => Box::pin(check_vcs_permalinks::run(hook, filenames)),
            Self::FileContentsSorter => Box::pin(file_contents_sorter::run(hook, filenames)),
            Self::EndOfFileFixer => Box::pin(fix_end_of_file::run(hook, filenames)),
            Self::FixByteOrderMarker => Box::pin(fix_byte_order_marker::run(hook, filenames)),
            Self::ForbidNewSubmodules => Box::pin(forbid_new_submodules::run(hook, filenames)),
            Self::CheckJson => Box::pin(check_json::run(hook, filenames)),
            Self::CheckSymlinks => Box::pin(check_symlinks::run(hook, filenames)),
            Self::CheckMergeConflict => Box::pin(check_merge_conflict::run(hook, filenames)),
            Self::CheckToml => Box::pin(check_toml::run(hook, filenames)),
            Self::CheckYaml => Box::pin(check_yaml::run(hook, filenames)),
            Self::CheckXml => Box::pin(check_xml::run(hook, filenames)),
            Self::DestroyedSymlinks => Box::pin(destroyed_symlinks::run(hook, filenames)),
            Self::MixedLineEnding => Box::pin(mixed_line_ending::run(hook, filenames)),
            Self::DetectPrivateKey => Box::pin(detect_private_key::run(hook, filenames)),
            Self::NoCommitToBranch => Box::pin(no_commit_to_branch::run(hook)),
            Self::RequirementsTxtFixer => Box::pin(requirements_txt_fixer::run(hook, filenames)),
            Self::TrailingWhitespace => Box::pin(fix_trailing_whitespace::run(hook, filenames)),
        };
        future.await
    }
}

// TODO: compare rev
pub(crate) fn is_pre_commit_hooks(url: &str) -> bool {
    url == "https://github.com/pre-commit/pre-commit-hooks"
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::*;

    #[tokio::test(flavor = "current_thread")]
    async fn explicit_filenames_run_serially_before_selected_filenames() {
        let explicit = vec![PathBuf::from("shared"), PathBuf::from("shared")];
        let selected = [Path::new("shared"), Path::new("selected")];
        let events = Rc::new(RefCell::new(Vec::new()));

        run_file_checks(&explicit, &selected, 2, |path| {
            let events = Rc::clone(&events);
            async move {
                events.borrow_mut().push(format!("start {path:?}"));
                tokio::task::yield_now().await;
                events.borrow_mut().push(format!("end {path:?}"));
                Ok(HookOutput::unchanged(0, Vec::new()))
            }
        })
        .await
        .unwrap();

        let events = events.borrow();
        assert_eq!(events.len(), 8);
        assert_eq!(
            &events[..4],
            [
                "start \"shared\"",
                "end \"shared\"",
                "start \"shared\"",
                "end \"shared\"",
            ]
        );
    }
}
