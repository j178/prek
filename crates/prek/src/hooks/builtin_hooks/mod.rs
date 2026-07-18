use std::path::Path;
use std::str::FromStr;

use anyhow::Result;
use prek_identify::tags;

use crate::cli::run::HookRunReporter;
use crate::config::{BuiltinHook, FilePattern, HookOptions, PassFilenames, Stage};
use crate::hook::Hook;
use crate::hooks::pre_commit_hooks;
use crate::store::Store;

use super::HookFuture;

mod check_illegal_windows_names;
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
    RequirePattern,
    TrailingWhitespace,
}

impl BuiltinHooks {
    pub(crate) fn may_modify_files(self) -> bool {
        match self {
            Self::EndOfFileFixer
            | Self::FileContentsSorter
            | Self::FixByteOrderMarker
            | Self::MixedLineEnding
            | Self::PrettyFormatJson
            | Self::TrailingWhitespace => true,

            Self::CheckAddedLargeFiles
            | Self::CheckCaseConflict
            | Self::CheckExecutablesHaveShebangs
            | Self::CheckIllegalWindowsNames
            | Self::CheckJson
            | Self::CheckJson5
            | Self::CheckMergeConflict
            | Self::CheckShebangScriptsAreExecutable
            | Self::CheckSymlinks
            | Self::CheckToml
            | Self::CheckVcsPermalinks
            | Self::CheckXml
            | Self::CheckYaml
            | Self::DenyPattern
            | Self::DestroyedSymlinks
            | Self::DetectPrivateKey
            | Self::ForbidNewSubmodules
            | Self::NoCommitToBranch
            | Self::RequirePattern => false,
        }
    }

    pub(crate) async fn run(
        self,
        _store: &Store,
        hook: &Hook,
        filenames: &[&Path],
        reporter: &HookRunReporter,
    ) -> Result<(i32, Vec<u8>)> {
        let progress = reporter.on_run_start(hook, filenames.len());
        let future: HookFuture<'_> = match self {
            Self::CheckAddedLargeFiles => {
                Box::pin(pre_commit_hooks::check_added_large_files(hook, filenames))
            }
            Self::CheckCaseConflict => {
                Box::pin(pre_commit_hooks::check_case_conflict(hook, filenames))
            }
            Self::CheckExecutablesHaveShebangs => Box::pin(
                pre_commit_hooks::check_executables_have_shebangs(hook, filenames),
            ),
            Self::CheckIllegalWindowsNames => Box::pin(std::future::ready(Ok(
                check_illegal_windows_names::check_illegal_windows_names(hook, filenames),
            ))),
            Self::CheckJson => Box::pin(pre_commit_hooks::check_json(hook, filenames)),
            Self::CheckJson5 => Box::pin(check_json5::check_json5(hook, filenames)),
            Self::CheckMergeConflict => {
                Box::pin(pre_commit_hooks::check_merge_conflict(hook, filenames))
            }
            Self::CheckShebangScriptsAreExecutable => Box::pin(
                pre_commit_hooks::check_shebang_scripts_are_executable(hook, filenames),
            ),
            Self::CheckSymlinks => Box::pin(pre_commit_hooks::check_symlinks(hook, filenames)),
            Self::CheckToml => Box::pin(pre_commit_hooks::check_toml(hook, filenames)),
            Self::CheckVcsPermalinks => {
                Box::pin(pre_commit_hooks::check_vcs_permalinks(hook, filenames))
            }
            Self::CheckXml => Box::pin(pre_commit_hooks::check_xml(hook, filenames)),
            Self::CheckYaml => Box::pin(pre_commit_hooks::check_yaml(hook, filenames)),
            Self::DenyPattern => Box::pin(pattern::deny_pattern(hook, filenames)),
            Self::DestroyedSymlinks => {
                Box::pin(pre_commit_hooks::destroyed_symlinks(hook, filenames))
            }
            Self::DetectPrivateKey => {
                Box::pin(pre_commit_hooks::detect_private_key(hook, filenames))
            }
            Self::EndOfFileFixer => Box::pin(pre_commit_hooks::fix_end_of_file(hook, filenames)),
            Self::FileContentsSorter => {
                Box::pin(pre_commit_hooks::file_contents_sorter(hook, filenames))
            }
            Self::FixByteOrderMarker => {
                Box::pin(pre_commit_hooks::fix_byte_order_marker(hook, filenames))
            }
            Self::ForbidNewSubmodules => {
                Box::pin(pre_commit_hooks::forbid_new_submodules(hook, filenames))
            }
            Self::MixedLineEnding => Box::pin(pre_commit_hooks::mixed_line_ending(hook, filenames)),
            Self::NoCommitToBranch => Box::pin(pre_commit_hooks::no_commit_to_branch(hook)),
            Self::PrettyFormatJson => {
                Box::pin(pre_commit_hooks::pretty_format_json(hook, filenames))
            }
            Self::RequirePattern => Box::pin(pattern::require_pattern(hook, filenames)),
            Self::TrailingWhitespace => {
                Box::pin(pre_commit_hooks::fix_trailing_whitespace(hook, filenames))
            }
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
                    description: Some("prevents giant files from being committed.".to_string()),
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
                        "checks for files that would conflict in case-insensitive filesystems"
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
                        "ensures that (non-binary) executables have a shebang.".to_string(),
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
                        "checks for filenames which cannot be created on Windows.".to_string(),
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
                    description: Some("checks json files for parseable syntax.".to_string()),
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
                    description: Some("checks json5 files for parseable syntax.".to_string()),
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
                        "checks for files that contain merge conflict strings.".to_string(),
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
                        "ensures that (non-binary) files with a shebang are executable."
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
                        "checks for symlinks which do not point to anything.".to_string(),
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
                    description: Some("checks toml files for parseable syntax.".to_string()),
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
                        "ensures that links to vcs websites are permalinks.".to_string(),
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
                    description: Some("checks xml files for parseable syntax.".to_string()),
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
                    description: Some("checks yaml files for parseable syntax.".to_string()),
                    types: Some(tags::TAG_SET_YAML),
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
                        "fails if any file contains a matching regular expression.".to_string(),
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
                        "detects symlinks that were replaced with regular files whose contents are the original symlink target path.".to_string(),
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
                    description: Some("detects the presence of private keys.".to_string()),
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
                        "ensures that a file is either empty, or ends with one newline."
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
                        "sorts the lines in specified files (defaults to alphabetical)."
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
                    description: Some("removes utf-8 byte order marker.".to_string()),
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
                    description: Some("Prevent addition of new git submodules.".to_string()),
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
                    description: Some("replaces or checks mixed line ending.".to_string()),
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
                    description: Some("checks that JSON files are pretty-formatted.".to_string()),
                    types: Some(tags::TAG_SET_JSON),
                    stages: Some([Stage::PreCommit, Stage::PrePush, Stage::Manual].into()),
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
                        "fails if any file does not contain a matching regular expression."
                            .to_string(),
                    ),
                    types: Some(tags::TAG_SET_TEXT),
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
                    description: Some("trims trailing whitespace.".to_string()),
                    types: Some(tags::TAG_SET_TEXT),
                    stages: Some([Stage::PreCommit, Stage::PrePush, Stage::Manual].into()),
                    ..Default::default()
                },
            },
        })
    }
}
