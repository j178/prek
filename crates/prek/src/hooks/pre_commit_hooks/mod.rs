use std::path::{Path, PathBuf};

use anyhow::Result;
use clap::Parser;
use tracing::debug;

use crate::hook::Hook;
use crate::hooks::HookFuture;
use crate::hooks::HookOutput;

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

#[derive(Parser)]
#[command(disable_help_subcommand = true)]
#[command(disable_version_flag = true)]
#[command(disable_help_flag = true)]
pub(crate) struct FixArgs {
    /// Report files that would change without modifying them.
    #[arg(long)]
    pub(crate) check: bool,
    #[arg(value_name = "FILENAMES")]
    pub(crate) filenames: Vec<PathBuf>,
}

pub(crate) fn contents_equal<'a>(
    mut original: &[u8],
    chunks: impl IntoIterator<Item = &'a [u8]>,
) -> bool {
    for chunk in chunks {
        let Some(rest) = original.strip_prefix(chunk) else {
            return false;
        };
        original = rest;
    }
    original.is_empty()
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
        debug!(
            "Running hook `{}` with prek's bundled Rust implementation instead of `{}` (fast path); behavior and defaults may differ. Set `language: {}` for this hook or `PREK_NO_FAST_PATH=1` to run the pinned implementation",
            hook.id,
            hook.repo(),
            hook.language,
        );
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
