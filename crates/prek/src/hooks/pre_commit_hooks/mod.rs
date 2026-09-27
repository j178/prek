use std::path::{Path, PathBuf};

use anyhow::Result;
use clap::Parser;
use rayon::iter::{IntoParallelRefIterator, ParallelIterator};
use tracing::debug;

use crate::hook::Hook;
use crate::hooks::HookOutput;

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

pub(crate) fn parse_hook_args<T: Parser>(hook: &Hook) -> Result<T> {
    Ok(T::try_parse_from(
        hook.entry.expect_argv_entry().split_with_args(&hook.args)?,
    )?)
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

pub(crate) fn hook_filenames<'a>(
    configured: &'a [PathBuf],
    selected: &'a [&Path],
) -> impl Iterator<Item = &'a Path> + 'a {
    configured
        .iter()
        .map(PathBuf::as_path)
        .chain(selected.iter().copied())
}

/// Runs blocking file checks, preserving filename order in the combined output.
/// Explicit filenames finish serially before selected filenames run in parallel.
pub(crate) async fn run_blocking_file_checks<F>(
    file_base: &Path,
    explicit: &[PathBuf],
    selected: &[&Path],
    check: F,
) -> Result<HookOutput>
where
    F: Fn(&Path, &Path) -> Result<HookOutput> + Send + Sync + 'static,
{
    let file_base = file_base.to_path_buf();
    let explicit_len = explicit.len();
    let filenames: Vec<_> = hook_filenames(explicit, selected)
        .map(Path::to_path_buf)
        .collect();
    // Enter the blocking pool once for the batch, rather than once per file or I/O operation.
    tokio::task::spawn_blocking(move || {
        let (explicit, selected) = filenames.split_at(explicit_len);
        let mut result = HookOutput::unchanged(0, Vec::new());
        for filename in explicit {
            result.merge_known(check(&file_base.join(filename), filename)?);
        }
        // Collect results in input order so diagnostics and the first I/O error are deterministic.
        let outputs: Vec<_> = selected
            .par_iter()
            .map(|filename| check(&file_base.join(filename), filename))
            .collect();
        for output in outputs {
            result.merge_known(output?);
        }
        Ok(result)
    })
    .await?
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn blocking_checks_finish_explicit_duplicates_before_selected_files() -> Result<()> {
        let dir = tempfile::tempdir()?;
        fs_err::write(dir.path().join("shared"), b"0")?;
        fs_err::write(dir.path().join("other"), b"9")?;
        let explicit = [PathBuf::from("shared"), PathBuf::from("shared")];
        let selected = [Path::new("shared"), Path::new("other")];
        let result = run_blocking_file_checks(dir.path(), &explicit, &selected, |path, _| {
            let contents = fs_err::read(path)?;
            fs_err::write(path, [contents[0] + 1])?;
            Ok(HookOutput::known(1, contents, true))
        })
        .await?;

        assert_eq!(result.output, b"0129");
        assert_eq!(result.exit_status, 1);
        assert_eq!(result.file_changes, crate::hooks::FileChanges::Modified);
        assert_eq!(fs_err::read(dir.path().join("shared"))?, b"3");
        Ok(())
    }

    #[tokio::test]
    async fn blocking_checks_preserve_input_order_with_overlapping_checks() -> Result<()> {
        // A single worker cannot overlap these checks.
        if rayon::current_num_threads() < 2 {
            return Ok(());
        }
        let (send, recv) = std::sync::mpsc::channel();
        let recv = std::sync::Mutex::new(recv);
        let selected = [Path::new("first"), Path::new("second")];
        let result = run_blocking_file_checks(Path::new(""), &[], &selected, move |_, name| {
            if name == Path::new("first") {
                recv.lock()
                    .map_err(|error| anyhow::anyhow!("{error}"))?
                    .recv()?;
                Ok(HookOutput::unchanged(1, b"first".to_vec()))
            } else {
                send.send(())?;
                Ok(HookOutput::unchanged(2, b"second".to_vec()))
            }
        })
        .await?;
        assert_eq!(result.output, b"firstsecond");
        assert_eq!(result.exit_status, 3);
        assert_eq!(result.file_changes, crate::hooks::FileChanges::Unchanged);
        Ok(())
    }
}
