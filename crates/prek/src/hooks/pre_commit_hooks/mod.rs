use std::path::Path;

use anyhow::Result;
use tracing::debug;

use crate::hook::Hook;

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
