use std::path::Path;
use std::str::FromStr;

use anyhow::Result;
use clap::{Command, CommandFactory};
use prek_identify::tags;

use crate::cli::run::HookRunReporter;
use crate::config::{BuiltinHook, FilePattern, HookOptions, PassFilenames, Stage};
use crate::hook::Hook;
use crate::hooks::pre_commit_hooks::{
    check_added_large_files, check_case_conflict, check_executables_have_shebangs,
    check_illegal_windows_names, check_json, check_merge_conflict,
    check_shebang_scripts_are_executable, check_symlinks, check_toml, check_vcs_permalinks,
    check_xml, check_yaml, destroyed_symlinks, detect_private_key, file_contents_sorter,
    fix_byte_order_marker, fix_end_of_file, fix_trailing_whitespace, forbid_new_submodules,
    mixed_line_ending, no_commit_to_branch, pretty_format_json, requirements_txt_fixer,
};
use crate::store::Store;

use super::{HookFuture, HookOutput};

mod check_json5;
mod pattern;

#[derive(
    Debug,
    Copy,
    Clone,
    PartialEq,
    Eq,
    strum::AsRefStr,
    strum::Display,
    strum::EnumIter,
    strum::EnumString,
)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "schemars", schemars(rename_all = "kebab-case"))]
#[strum(serialize_all = "kebab-case")]
pub(crate) enum BuiltinHooks {
    CheckAddedLargeFiles,
    CheckCaseConflict,
    CheckExecutablesHaveShebangs,
    CheckIllegalWindowsNames,
    CheckJson,
    CheckJson5,
    CheckMergeConflict,
    CheckShebangScriptsAreExecutable,
    CheckSymlinks,
    CheckToml,
    CheckVcsPermalinks,
    CheckXml,
    CheckYaml,
    DenyFilenamePattern,
    DenyPattern,
    DestroyedSymlinks,
    DetectPrivateKey,
    EndOfFileFixer,
    FileContentsSorter,
    FixByteOrderMarker,
    ForbidNewSubmodules,
    MixedLineEnding,
    NoCommitToBranch,
    PrettyFormatJson,
    RequireFilenamePattern,
    RequirePattern,
    RequirementsTxtFixer,
    TrailingWhitespace,
}

impl BuiltinHooks {
    fn flags_command(self) -> Option<Command> {
        Some(match self {
            Self::CheckAddedLargeFiles => check_added_large_files::Args::command(),
            Self::CheckMergeConflict => check_merge_conflict::Args::command(),
            Self::CheckVcsPermalinks => check_vcs_permalinks::Args::command(),
            Self::CheckYaml => check_yaml::Args::command(),
            Self::DenyFilenamePattern | Self::RequireFilenamePattern => {
                pattern::FilenameArgs::command()
            }
            Self::DenyPattern | Self::RequirePattern => pattern::Args::command(),
            Self::FileContentsSorter => file_contents_sorter::Args::command(),
            Self::MixedLineEnding => mixed_line_ending::Args::command(),
            Self::NoCommitToBranch => no_commit_to_branch::Args::command(),
            Self::PrettyFormatJson => pretty_format_json::Args::command(),
            Self::TrailingWhitespace => fix_trailing_whitespace::Args::command(),
            _ => return None,
        })
    }

    pub(crate) fn flags_help(self) -> Option<String> {
        let mut command = self.flags_command()?.help_template("{options}");
        let help = command.render_help().to_string();
        if help.is_empty() { None } else { Some(help) }
    }

    pub(crate) async fn run(
        self,
        _store: &Store,
        hook: &Hook,
        filenames: &[&Path],
        reporter: &HookRunReporter,
    ) -> Result<HookOutput> {
        let progress = reporter.on_run_start(hook, filenames.len());
        let future: HookFuture<'_> = match self {
            Self::CheckAddedLargeFiles => Box::pin(check_added_large_files::run(hook, filenames)),
            Self::CheckCaseConflict => Box::pin(check_case_conflict::run(hook, filenames)),
            Self::CheckExecutablesHaveShebangs => {
                Box::pin(check_executables_have_shebangs::run(hook, filenames))
            }
            Self::CheckIllegalWindowsNames => {
                Box::pin(check_illegal_windows_names::run(hook, filenames))
            }
            Self::CheckJson => Box::pin(check_json::run(hook, filenames)),
            Self::CheckJson5 => Box::pin(check_json5::check_json5(hook, filenames)),
            Self::CheckMergeConflict => Box::pin(check_merge_conflict::run(hook, filenames)),
            Self::CheckShebangScriptsAreExecutable => {
                Box::pin(check_shebang_scripts_are_executable::run(hook, filenames))
            }
            Self::CheckSymlinks => Box::pin(check_symlinks::run(hook, filenames)),
            Self::CheckToml => Box::pin(check_toml::run(hook, filenames)),
            Self::CheckVcsPermalinks => Box::pin(check_vcs_permalinks::run(hook, filenames)),
            Self::CheckXml => Box::pin(check_xml::run(hook, filenames)),
            Self::CheckYaml => Box::pin(check_yaml::run(hook, filenames)),
            Self::DenyFilenamePattern => Box::pin(std::future::ready(
                pattern::deny_filename_pattern(hook, filenames),
            )),
            Self::DenyPattern => Box::pin(pattern::deny_pattern(hook, filenames)),
            Self::DestroyedSymlinks => Box::pin(destroyed_symlinks::run(hook, filenames)),
            Self::DetectPrivateKey => Box::pin(detect_private_key::run(hook, filenames)),
            Self::EndOfFileFixer => Box::pin(fix_end_of_file::run(hook, filenames)),
            Self::FileContentsSorter => Box::pin(file_contents_sorter::run(hook, filenames)),
            Self::FixByteOrderMarker => Box::pin(fix_byte_order_marker::run(hook, filenames)),
            Self::ForbidNewSubmodules => Box::pin(forbid_new_submodules::run(hook, filenames)),
            Self::MixedLineEnding => Box::pin(mixed_line_ending::run(hook, filenames)),
            Self::NoCommitToBranch => Box::pin(no_commit_to_branch::run(hook)),
            Self::PrettyFormatJson => Box::pin(pretty_format_json::run(hook, filenames)),
            Self::RequireFilenamePattern => Box::pin(std::future::ready(
                pattern::require_filename_pattern(hook, filenames),
            )),
            Self::RequirePattern => Box::pin(pattern::require_pattern(hook, filenames)),
            Self::RequirementsTxtFixer => Box::pin(requirements_txt_fixer::run(hook, filenames)),
            Self::TrailingWhitespace => Box::pin(fix_trailing_whitespace::run(hook, filenames)),
        };
        let result = future.await;
        reporter.on_run_complete(progress);
        result
    }
}

impl BuiltinHook {
    pub(crate) fn from_id(id: &str) -> Result<Self, ()> {
        let hook_id = BuiltinHooks::from_str(id).map_err(|_| ())?;
        Ok(match hook_id {
            BuiltinHooks::CheckAddedLargeFiles => BuiltinHook {
                id: "check-added-large-files".to_string(),
                name: "check for added large files".to_string(),
                entry: "check-added-large-files".to_string(),
                priority: None,
                groups: None,
                options: HookOptions {
                    description: Some("Prevents giant files from being committed.".to_string()),
                    stages: Some([Stage::PreCommit, Stage::PrePush, Stage::Manual].into()),
                    ..Default::default()
                },
            },
            BuiltinHooks::CheckCaseConflict => BuiltinHook {
                id: "check-case-conflict".to_string(),
                name: "check for case conflicts".to_string(),
                entry: "check-case-conflict".to_string(),
                priority: None,
                groups: None,
                options: HookOptions {
                    description: Some(
                        "Checks for files that would conflict in case-insensitive filesystems."
                            .to_string(),
                    ),
                    ..Default::default()
                },
            },
            BuiltinHooks::CheckExecutablesHaveShebangs => BuiltinHook {
                id: "check-executables-have-shebangs".to_string(),
                name: "check that executables have shebangs".to_string(),
                entry: "check-executables-have-shebangs".to_string(),
                priority: None,
                groups: None,
                options: HookOptions {
                    description: Some(
                        "Ensures that (non-binary) executables have a shebang.".to_string(),
                    ),
                    types: Some(tags::TAG_SET_EXECUTABLE_TEXT),
                    stages: Some([Stage::PreCommit, Stage::PrePush, Stage::Manual].into()),
                    ..Default::default()
                },
            },
            BuiltinHooks::CheckIllegalWindowsNames => BuiltinHook {
                id: "check-illegal-windows-names".to_string(),
                name: "check illegal windows names".to_string(),
                entry: "check-illegal-windows-names".to_string(),
                priority: None,
                groups: None,
                options: HookOptions {
                    description: Some(
                        "Checks for filenames which cannot be created on Windows.".to_string(),
                    ),
                    files: Some(
                        FilePattern::regex(
                            check_illegal_windows_names::ILLEGAL_WINDOWS_PATTERN,
                        )
                        .expect("builtin files regex must be valid"),
                    ),
                    ..Default::default()
                },
            },
            BuiltinHooks::CheckJson => BuiltinHook {
                id: "check-json".to_string(),
                name: "check json".to_string(),
                entry: "check-json".to_string(),
                priority: None,
                groups: None,
                options: HookOptions {
                    description: Some("Checks JSON files for parseable syntax.".to_string()),
                    types: Some(tags::TAG_SET_JSON),
                    ..Default::default()
                },
            },
            BuiltinHooks::CheckJson5 => BuiltinHook {
                id: "check-json5".to_string(),
                name: "check json5".to_string(),
                entry: "check-json5".to_string(),
                priority: None,
                groups: None,
                options: HookOptions {
                    description: Some("Checks JSON5 files for parseable syntax.".to_string()),
                    types: Some(tags::TAG_SET_JSON5),
                    ..Default::default()
                },
            },
            BuiltinHooks::CheckMergeConflict => BuiltinHook {
                id: "check-merge-conflict".to_string(),
                name: "check for merge conflicts".to_string(),
                entry: "check-merge-conflict".to_string(),
                priority: None,
                groups: None,
                options: HookOptions {
                    description: Some(
                        "Checks for files that contain merge conflict strings.".to_string(),
                    ),
                    types: Some(tags::TAG_SET_TEXT),
                    ..Default::default()
                },
            },
            BuiltinHooks::CheckShebangScriptsAreExecutable => BuiltinHook {
                id: "check-shebang-scripts-are-executable".to_string(),
                name: "check that scripts with shebangs are executable".to_string(),
                entry: "check-shebang-scripts-are-executable".to_string(),
                priority: None,
                groups: None,
                options: HookOptions {
                    description: Some(
                        "Ensures that (non-binary) files with a shebang are executable."
                            .to_string(),
                    ),
                    types: Some(tags::TAG_SET_TEXT),
                    stages: Some([Stage::PreCommit, Stage::PrePush, Stage::Manual].into()),
                    ..Default::default()
                },
            },
            BuiltinHooks::CheckSymlinks => BuiltinHook {
                id: "check-symlinks".to_string(),
                name: "check for broken symlinks".to_string(),
                entry: "check-symlinks".to_string(),
                priority: None,
                groups: None,
                options: HookOptions {
                    description: Some(
                        "Checks for symlinks which do not point to anything.".to_string(),
                    ),
                    types: Some(tags::TAG_SET_SYMLINK),
                    ..Default::default()
                },
            },
            BuiltinHooks::CheckToml => BuiltinHook {
                id: "check-toml".to_string(),
                name: "check toml".to_string(),
                entry: "check-toml".to_string(),
                priority: None,
                groups: None,
                options: HookOptions {
                    description: Some("Checks TOML files for parseable syntax.".to_string()),
                    types: Some(tags::TAG_SET_TOML),
                    ..Default::default()
                },
            },
            BuiltinHooks::CheckVcsPermalinks => BuiltinHook {
                id: "check-vcs-permalinks".to_string(),
                name: "check vcs permalinks".to_string(),
                entry: "check-vcs-permalinks".to_string(),
                priority: None,
                groups: None,
                options: HookOptions {
                    description: Some(
                        "Ensures that links to VCS websites are permalinks.".to_string(),
                    ),
                    types: Some(tags::TAG_SET_TEXT),
                    ..Default::default()
                },
            },
            BuiltinHooks::CheckXml => BuiltinHook {
                id: "check-xml".to_string(),
                name: "check xml".to_string(),
                entry: "check-xml".to_string(),
                priority: None,
                groups: None,
                options: HookOptions {
                    description: Some("Checks XML files for parseable syntax.".to_string()),
                    types: Some(tags::TAG_SET_XML),
                    ..Default::default()
                },
            },
            BuiltinHooks::CheckYaml => BuiltinHook {
                id: "check-yaml".to_string(),
                name: "check yaml".to_string(),
                entry: "check-yaml".to_string(),
                priority: None,
                groups: None,
                options: HookOptions {
                    description: Some("Checks YAML files for parseable syntax.".to_string()),
                    types: Some(tags::TAG_SET_YAML),
                    ..Default::default()
                },
            },
            BuiltinHooks::DenyFilenamePattern => BuiltinHook {
                id: "deny-filename-pattern".to_string(),
                name: "deny filename patterns".to_string(),
                entry: "deny-filename-pattern".to_string(),
                priority: None,
                groups: None,
                options: HookOptions {
                    description: Some(
                        "Fails if any selected filename matches a regular expression."
                            .to_string(),
                    ),
                    ..Default::default()
                },
            },
            BuiltinHooks::DenyPattern => BuiltinHook {
                id: "deny-pattern".to_string(),
                name: "deny patterns".to_string(),
                entry: "deny-pattern".to_string(),
                priority: None,
                groups: None,
                options: HookOptions {
                    description: Some(
                        "Fails if any file contains a matching regular expression.".to_string(),
                    ),
                    types: Some(tags::TAG_SET_TEXT),
                    ..Default::default()
                },
            },
            BuiltinHooks::DestroyedSymlinks => BuiltinHook {
                id: "destroyed-symlinks".to_string(),
                name: "detect destroyed symlinks".to_string(),
                entry: "destroyed-symlinks".to_string(),
                priority: None,
                groups: None,
                options: HookOptions {
                    description: Some(
                        "Detects symlinks that were replaced with regular files whose contents are the original symlink target path.".to_string(),
                    ),
                    types: Some(tags::TAG_SET_FILE),
                    stages: Some([Stage::PreCommit, Stage::PrePush, Stage::Manual].into()),
                    ..Default::default()
                },
            },
            BuiltinHooks::DetectPrivateKey => BuiltinHook {
                id: "detect-private-key".to_string(),
                name: "detect private key".to_string(),
                entry: "detect-private-key".to_string(),
                priority: None,
                groups: None,
                options: HookOptions {
                    description: Some("Detects the presence of private keys.".to_string()),
                    types: Some(tags::TAG_SET_TEXT),
                    ..Default::default()
                },
            },
            BuiltinHooks::EndOfFileFixer => BuiltinHook {
                id: "end-of-file-fixer".to_string(),
                name: "fix end of files".to_string(),
                entry: "end-of-file-fixer".to_string(),
                priority: None,
                groups: None,
                options: HookOptions {
                    description: Some(
                        "Ensures that a file is either empty, or ends with one newline."
                            .to_string(),
                    ),
                    types: Some(tags::TAG_SET_TEXT),
                    stages: Some([Stage::PreCommit, Stage::PrePush, Stage::Manual].into()),
                    ..Default::default()
                },
            },
            BuiltinHooks::FileContentsSorter => BuiltinHook {
                id: "file-contents-sorter".to_string(),
                name: "file contents sorter".to_string(),
                entry: "file-contents-sorter".to_string(),
                priority: None,
                groups: None,
                options: HookOptions {
                    description: Some(
                        "Sorts the lines in specified files (defaults to alphabetical)."
                            .to_string(),
                    ),
                    files: Some(FilePattern::Never),
                    ..Default::default()
                },
            },
            BuiltinHooks::FixByteOrderMarker => BuiltinHook {
                id: "fix-byte-order-marker".to_string(),
                name: "fix utf-8 byte order marker".to_string(),
                entry: "fix-byte-order-marker".to_string(),
                priority: None,
                groups: None,
                options: HookOptions {
                    description: Some("Removes UTF-8 byte order marker.".to_string()),
                    types: Some(tags::TAG_SET_TEXT),
                    ..Default::default()
                },
            },
            BuiltinHooks::ForbidNewSubmodules => BuiltinHook {
                 id: "forbid-new-submodules".to_string(),
                 name: "forbid new submodules".to_string(),
                 entry: "forbid-new-submodules".to_string(),
                 priority: None,
                 groups: None,
                 options: HookOptions {
                    description: Some("Prevents the addition of new Git submodules.".to_string()),
                    types: Some(tags::TAG_SET_DIRECTORY),
                    ..Default::default()
                 },
            },
            BuiltinHooks::MixedLineEnding => BuiltinHook {
                id: "mixed-line-ending".to_string(),
                name: "mixed line ending".to_string(),
                entry: "mixed-line-ending".to_string(),
                priority: None,
                groups: None,
                options: HookOptions {
                    description: Some("Replaces or checks mixed line endings.".to_string()),
                    types: Some(tags::TAG_SET_TEXT),
                    ..Default::default()
                },
            },
            BuiltinHooks::NoCommitToBranch => BuiltinHook {
                id: "no-commit-to-branch".to_string(),
                name: "don't commit to branch".to_string(),
                entry: "no-commit-to-branch".to_string(),
                priority: None,
                groups: None,
                options: HookOptions {
                    description: Some(
                        "Protects specific branches from direct commits.".to_string(),
                    ),
                    pass_filenames: Some(PassFilenames::None),
                    always_run: Some(true),
                    ..Default::default()
                },
            },
            BuiltinHooks::PrettyFormatJson => BuiltinHook {
                id: "pretty-format-json".to_string(),
                name: "pretty format json".to_string(),
                entry: "pretty-format-json".to_string(),
                priority: None,
                groups: None,
                options: HookOptions {
                    description: Some("Checks that JSON files are pretty-formatted.".to_string()),
                    types: Some(tags::TAG_SET_JSON),
                    stages: Some([Stage::PreCommit, Stage::PrePush, Stage::Manual].into()),
                    ..Default::default()
                },
            },
            BuiltinHooks::RequireFilenamePattern => BuiltinHook {
                id: "require-filename-pattern".to_string(),
                name: "require filename patterns".to_string(),
                entry: "require-filename-pattern".to_string(),
                priority: None,
                groups: None,
                options: HookOptions {
                    description: Some(
                        "Fails if any selected filename does not match a regular expression."
                            .to_string(),
                    ),
                    ..Default::default()
                },
            },
            BuiltinHooks::RequirePattern => BuiltinHook {
                id: "require-pattern".to_string(),
                name: "require patterns".to_string(),
                entry: "require-pattern".to_string(),
                priority: None,
                groups: None,
                options: HookOptions {
                    description: Some(
                        "Fails if any file does not contain a matching regular expression."
                            .to_string(),
                    ),
                    types: Some(tags::TAG_SET_TEXT),
                    ..Default::default()
                },
            },
            BuiltinHooks::RequirementsTxtFixer => BuiltinHook {
                id: "requirements-txt-fixer".to_string(),
                name: "fix requirements.txt".to_string(),
                entry: "requirements-txt-fixer".to_string(),
                priority: None,
                groups: None,
                options: HookOptions {
                    description: Some("Sorts entries in requirements.txt.".to_string()),
                    files: Some(
                        FilePattern::regex(r"(requirements|constraints).*\.txt$")
                            .expect("builtin files regex must be valid"),
                    ),
                    ..Default::default()
                },
            },
            BuiltinHooks::TrailingWhitespace => BuiltinHook {
                id: "trailing-whitespace".to_string(),
                name: "trim trailing whitespace".to_string(),
                entry: "trailing-whitespace-fixer".to_string(),
                priority: None,
                groups: None,
                options: HookOptions {
                    description: Some("Trims trailing whitespace.".to_string()),
                    types: Some(tags::TAG_SET_TEXT),
                    stages: Some([Stage::PreCommit, Stage::PrePush, Stage::Manual].into()),
                    ..Default::default()
                },
            },
        })
    }
}
