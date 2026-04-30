use std::path::Path;
use std::str::FromStr;

use anyhow::Result;
use prek_identify::tags;

use crate::cli::reporter::HookRunReporter;
use crate::config::{BuiltinHook, FilePattern, HookOptions, PassFilenames, Stage};
use crate::hook::Hook;
use crate::hooks::pre_commit_hooks;
use crate::store::Store;

mod check_illegal_windows_names;
mod check_json5;

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
    DestroyedSymlinks,
    DetectPrivateKey,
    EndOfFileFixer,
    FileContentsSorter,
    FixByteOrderMarker,
    ForbidNewSubmodules,
    MixedLineEnding,
    NoCommitToBranch,
    PrettyFormatJson,
    TrailingWhitespace,
}

impl BuiltinHooks {
    pub(crate) async fn run(
        self,
        _store: &Store,
        hook: &Hook,
        filenames: &[&Path],
        reporter: &HookRunReporter,
    ) -> Result<(i32, Vec<u8>)> {
        let progress = reporter.on_run_start(hook, filenames.len());
        let result = match self {
            Self::CheckAddedLargeFiles => {
                pre_commit_hooks::check_added_large_files(hook, filenames).await
            }
            Self::CheckCaseConflict => pre_commit_hooks::check_case_conflict(hook, filenames).await,
            Self::CheckExecutablesHaveShebangs => {
                pre_commit_hooks::check_executables_have_shebangs(hook, filenames).await
            }
            Self::CheckIllegalWindowsNames => Ok(
                check_illegal_windows_names::check_illegal_windows_names(hook, filenames),
            ),
            Self::CheckJson => pre_commit_hooks::check_json(hook, filenames).await,
            Self::CheckJson5 => check_json5::check_json5(hook, filenames).await,
            Self::CheckMergeConflict => {
                pre_commit_hooks::check_merge_conflict(hook, filenames).await
            }
            Self::CheckShebangScriptsAreExecutable => {
                pre_commit_hooks::check_shebang_scripts_are_executable(hook, filenames).await
            }
            Self::CheckSymlinks => pre_commit_hooks::check_symlinks(hook, filenames).await,
            Self::CheckToml => pre_commit_hooks::check_toml(hook, filenames).await,
            Self::CheckVcsPermalinks => {
                pre_commit_hooks::check_vcs_permalinks(hook, filenames).await
            }
            Self::CheckXml => pre_commit_hooks::check_xml(hook, filenames).await,
            Self::CheckYaml => pre_commit_hooks::check_yaml(hook, filenames).await,
            Self::DestroyedSymlinks => pre_commit_hooks::destroyed_symlinks(hook, filenames).await,
            Self::DetectPrivateKey => pre_commit_hooks::detect_private_key(hook, filenames).await,
            Self::EndOfFileFixer => pre_commit_hooks::fix_end_of_file(hook, filenames).await,
            Self::FileContentsSorter => {
                pre_commit_hooks::file_contents_sorter(hook, filenames).await
            }
            Self::FixByteOrderMarker => {
                pre_commit_hooks::fix_byte_order_marker(hook, filenames).await
            }
            Self::ForbidNewSubmodules => {
                pre_commit_hooks::forbid_new_submodules(hook, filenames).await
            }
            Self::MixedLineEnding => pre_commit_hooks::mixed_line_ending(hook, filenames).await,
            Self::NoCommitToBranch => pre_commit_hooks::no_commit_to_branch(hook).await,
            Self::PrettyFormatJson => pre_commit_hooks::pretty_format_json(hook, filenames).await,
            Self::TrailingWhitespace => {
                pre_commit_hooks::fix_trailing_whitespace(hook, filenames).await
            }
        };
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
                options: HookOptions {
                    description: Some("checks yaml files for parseable syntax.".to_string()),
                    types: Some(tags::TAG_SET_YAML),
                    ..Default::default()
                },
            },
            BuiltinHooks::DestroyedSymlinks => BuiltinHook {
                id: "destroyed-symlinks".to_string(),
                name: "detect destroyed symlinks".to_string(),
                entry: "destroyed-symlinks".to_string(),
                priority: None,
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
                options: HookOptions {
                    description: Some("checks that JSON files are pretty-formatted.".to_string()),
                    types: Some(tags::TAG_SET_JSON),
                    stages: Some([Stage::PreCommit, Stage::PrePush, Stage::Manual].into()),
                    ..Default::default()
                },
            },
            BuiltinHooks::TrailingWhitespace => BuiltinHook {
                id: "trailing-whitespace".to_string(),
                name: "trim trailing whitespace".to_string(),
                entry: "trailing-whitespace-fixer".to_string(),
                priority: None,
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
