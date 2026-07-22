use std::future::Future;
use std::path::{Path, PathBuf};

use anyhow::Result;
use clap::Parser;
use tracing::debug;

use crate::hook::Hook;
use crate::hooks::run_concurrent_file_checks;

use super::HookFuture;

mod check_added_large_files;
mod check_case_conflict;
mod check_executables_have_shebangs;
pub(crate) mod check_json;
mod check_merge_conflict;
mod check_shebang_scripts_are_executable;
mod check_symlinks;
mod check_toml;
mod check_vcs_permalinks;
mod check_xml;
mod check_yaml;
mod destroyed_symlinks;
mod detect_private_key;
mod file_contents_sorter;
mod fix_byte_order_marker;
mod fix_end_of_file;
mod fix_trailing_whitespace;
mod forbid_new_submodules;
mod mixed_line_ending;
mod no_commit_to_branch;
mod pretty_format_json;
mod shebangs;

pub(crate) use check_added_large_files::check_added_large_files;
pub(crate) use check_case_conflict::check_case_conflict;
pub(crate) use check_executables_have_shebangs::check_executables_have_shebangs;
pub(crate) use check_json::check_json;
pub(crate) use check_merge_conflict::check_merge_conflict;
pub(crate) use check_shebang_scripts_are_executable::check_shebang_scripts_are_executable;
pub(crate) use check_symlinks::check_symlinks;
pub(crate) use check_toml::check_toml;
pub(crate) use check_vcs_permalinks::check_vcs_permalinks;
pub(crate) use check_xml::check_xml;
pub(crate) use check_yaml::check_yaml;
pub(crate) use destroyed_symlinks::destroyed_symlinks;
pub(crate) use detect_private_key::detect_private_key;
pub(crate) use file_contents_sorter::file_contents_sorter;
pub(crate) use fix_byte_order_marker::fix_byte_order_marker;
pub(crate) use fix_end_of_file::fix_end_of_file;
pub(crate) use fix_trailing_whitespace::fix_trailing_whitespace;
pub(crate) use forbid_new_submodules::forbid_new_submodules;
pub(crate) use mixed_line_ending::mixed_line_ending;
pub(crate) use no_commit_to_branch::no_commit_to_branch;
pub(crate) use pretty_format_json::pretty_format_json;

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

pub(crate) async fn run_file_checks<'a, F, Fut>(
    explicit: &'a [PathBuf],
    selected: &'a [&Path],
    concurrency: usize,
    check: F,
) -> Result<(i32, Vec<u8>)>
where
    F: Fn(&'a Path) -> Fut,
    Fut: Future<Output = Result<(i32, Vec<u8>)>>,
{
    // Keep the common case on the existing concurrent path without an extra accumulator.
    if explicit.is_empty() {
        return run_concurrent_file_checks(selected.iter().copied(), concurrency, check).await;
    }

    // Filenames from `entry` or `args` may repeat or overlap with `selected`, so finish
    // them serially in CLI order before starting the selected batch.
    let mut code = 0;
    let mut output = Vec::new();
    for filename in explicit {
        let (file_code, file_output) = check(filename).await?;
        code |= file_code;
        output.extend(file_output);
    }
    if selected.is_empty() {
        return Ok((code, output));
    }

    // The explicit batch is complete, so selected filenames retain their normal concurrency.
    let (selected_code, selected_output) =
        run_concurrent_file_checks(selected.iter().copied(), concurrency, check).await?;
    code |= selected_code;
    output.extend(selected_output);
    Ok((code, output))
}

/// Hooks from `https://github.com/pre-commit/pre-commit-hooks`.
#[derive(strum::EnumString)]
#[strum(serialize_all = "kebab-case")]
pub(crate) enum PreCommitHooks {
    CheckAddedLargeFiles,
    CheckCaseConflict,
    CheckExecutablesHaveShebangs,
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
    // `pretty-format-json` is intentionally builtin-only for now. Do not enable
    // automatic fast-path replacement until parity coverage against upstream
    // Python is broad enough to trust it as the default implementation.
    // PrettyFormatJson,
    TrailingWhitespace,
}

impl PreCommitHooks {
    pub(crate) fn check_supported(&self, hook: &Hook) -> bool {
        match self {
            // `check-yaml` does not support `--unsafe` flag yet.
            Self::CheckYaml => !hook.args.iter().any(|s| s.starts_with("--unsafe")),
            _ => true,
        }
    }

    pub(crate) fn may_modify_files(&self) -> bool {
        match self {
            Self::EndOfFileFixer
            | Self::FileContentsSorter
            | Self::FixByteOrderMarker
            | Self::MixedLineEnding
            | Self::TrailingWhitespace => true,

            Self::CheckAddedLargeFiles
            | Self::CheckCaseConflict
            | Self::CheckExecutablesHaveShebangs
            | Self::CheckShebangScriptsAreExecutable
            | Self::CheckVcsPermalinks
            | Self::ForbidNewSubmodules
            | Self::CheckJson
            | Self::CheckSymlinks
            | Self::CheckMergeConflict
            | Self::CheckToml
            | Self::CheckXml
            | Self::CheckYaml
            | Self::DestroyedSymlinks
            | Self::DetectPrivateKey
            | Self::NoCommitToBranch => false,
        }
    }

    pub(crate) async fn run(self, hook: &Hook, filenames: &[&Path]) -> Result<(i32, Vec<u8>)> {
        debug!("Running hook `{}` in fast path", hook.id);
        let future: HookFuture<'_> = match self {
            Self::CheckAddedLargeFiles => Box::pin(check_added_large_files(hook, filenames)),
            Self::CheckCaseConflict => Box::pin(check_case_conflict(hook, filenames)),
            Self::CheckExecutablesHaveShebangs => {
                Box::pin(check_executables_have_shebangs(hook, filenames))
            }
            Self::CheckShebangScriptsAreExecutable => {
                Box::pin(check_shebang_scripts_are_executable(hook, filenames))
            }
            Self::CheckVcsPermalinks => Box::pin(check_vcs_permalinks(hook, filenames)),
            Self::FileContentsSorter => Box::pin(file_contents_sorter(hook, filenames)),
            Self::EndOfFileFixer => Box::pin(fix_end_of_file(hook, filenames)),
            Self::FixByteOrderMarker => Box::pin(fix_byte_order_marker(hook, filenames)),
            Self::ForbidNewSubmodules => Box::pin(forbid_new_submodules(hook, filenames)),
            Self::CheckJson => Box::pin(check_json(hook, filenames)),
            Self::CheckSymlinks => Box::pin(check_symlinks(hook, filenames)),
            Self::CheckMergeConflict => Box::pin(check_merge_conflict(hook, filenames)),
            Self::CheckToml => Box::pin(check_toml(hook, filenames)),
            Self::CheckYaml => Box::pin(check_yaml(hook, filenames)),
            Self::CheckXml => Box::pin(check_xml(hook, filenames)),
            Self::DestroyedSymlinks => Box::pin(destroyed_symlinks(hook, filenames)),
            Self::MixedLineEnding => Box::pin(mixed_line_ending(hook, filenames)),
            Self::DetectPrivateKey => Box::pin(detect_private_key(hook, filenames)),
            Self::NoCommitToBranch => Box::pin(no_commit_to_branch(hook)),
            Self::TrailingWhitespace => Box::pin(fix_trailing_whitespace(hook, filenames)),
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
                Ok((0, Vec::new()))
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
