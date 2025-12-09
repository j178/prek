use anyhow::Result;
use std::path::Path;
use std::str::FromStr;

use crate::config::{BuiltinHook, HookOptions, Language, ManifestHook, Stage};
use crate::hook::Hook;
use crate::hooks::pre_commit_hooks;
use crate::store::Store;

#[derive(Debug, Copy, Clone)]
pub(crate) enum BuiltinHooks {
    TrailingWhitespace,
    CheckAddedLargeFiles,
    EndOfFileFixer,
    FixByteOrderMarker,
    CheckJson,
    CheckSymlinks,
    CheckMergeConflict,
    CheckToml,
    CheckXml,
    CheckYaml,
    MixedLineEnding,
    DetectPrivateKey,
    NoCommitToBranch,
    CheckExecutablesHaveShebangs,
}

impl FromStr for BuiltinHooks {
    type Err = ();

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "trailing-whitespace" => Ok(Self::TrailingWhitespace),
            "check-added-large-files" => Ok(Self::CheckAddedLargeFiles),
            "end-of-file-fixer" => Ok(Self::EndOfFileFixer),
            "fix-byte-order-marker" => Ok(Self::FixByteOrderMarker),
            "check-json" => Ok(Self::CheckJson),
            "check-merge-conflict" => Ok(Self::CheckMergeConflict),
            "check-toml" => Ok(Self::CheckToml),
            "check-symlinks" => Ok(Self::CheckSymlinks),
            "check-xml" => Ok(Self::CheckXml),
            "check-yaml" => Ok(Self::CheckYaml),
            "mixed-line-ending" => Ok(Self::MixedLineEnding),
            "detect-private-key" => Ok(Self::DetectPrivateKey),
            "no-commit-to-branch" => Ok(Self::NoCommitToBranch),
            "check-executables-have-shebangs" => Ok(Self::CheckExecutablesHaveShebangs),
            _ => Err(()),
        }
    }
}

impl BuiltinHooks {
    pub(crate) async fn run(
        self,
        _store: &Store,
        hook: &Hook,
        filenames: &[&Path],
    ) -> Result<(i32, Vec<u8>)> {
        match self {
            Self::TrailingWhitespace => {
                pre_commit_hooks::fix_trailing_whitespace(hook, filenames).await
            }
            Self::CheckAddedLargeFiles => {
                pre_commit_hooks::check_added_large_files(hook, filenames).await
            }
            Self::EndOfFileFixer => pre_commit_hooks::fix_end_of_file(hook, filenames).await,
            Self::FixByteOrderMarker => {
                pre_commit_hooks::fix_byte_order_marker(hook, filenames).await
            }
            Self::CheckJson => pre_commit_hooks::check_json(hook, filenames).await,
            Self::CheckSymlinks => pre_commit_hooks::check_symlinks(hook, filenames).await,
            Self::CheckMergeConflict => {
                pre_commit_hooks::check_merge_conflict(hook, filenames).await
            }
            Self::CheckToml => pre_commit_hooks::check_toml(hook, filenames).await,
            Self::CheckYaml => pre_commit_hooks::check_yaml(hook, filenames).await,
            Self::CheckXml => pre_commit_hooks::check_xml(hook, filenames).await,
            Self::MixedLineEnding => pre_commit_hooks::mixed_line_ending(hook, filenames).await,
            Self::DetectPrivateKey => pre_commit_hooks::detect_private_key(hook, filenames).await,
            Self::NoCommitToBranch => pre_commit_hooks::no_commit_to_branch(hook).await,
            Self::CheckExecutablesHaveShebangs => {
                pre_commit_hooks::check_executables_have_shebangs(hook, filenames).await
            }
        }
    }
}

impl BuiltinHook {
    pub(crate) fn from_id(id: &str) -> Result<Self, ()> {
        let hook_id = BuiltinHooks::from_str(id)?;
        let hook = match hook_id {
            BuiltinHooks::TrailingWhitespace => ManifestHook {
                id: "trailing-whitespace".to_string(),
                name: "trim trailing whitespace".to_string(),
                language: Language::Python,
                entry: "trailing-whitespace-fixer".to_string(),
                options: HookOptions {
                    description: Some("trims trailing whitespace.".to_string()),
                    types: Some(vec!["text".to_string()]),
                    stages: Some(vec![Stage::PreCommit, Stage::PrePush, Stage::Manual]),
                    ..Default::default()
                },
            },
            BuiltinHooks::CheckAddedLargeFiles => ManifestHook {
                id: "check-added-large-files".to_string(),
                name: "check for added large files".to_string(),
                language: Language::Python,
                entry: "check-added-large-files".to_string(),
                options: HookOptions {
                    description: Some("prevents giant files from being committed.".to_string()),
                    stages: Some(vec![Stage::PreCommit, Stage::PrePush, Stage::Manual]),
                    ..Default::default()
                },
            },
            BuiltinHooks::EndOfFileFixer => ManifestHook {
                id: "end-of-file-fixer".to_string(),
                name: "fix end of files".to_string(),
                language: Language::Python,
                entry: "end-of-file-fixer".to_string(),
                options: HookOptions {
                    description: Some(
                        "ensures that a file is either empty, or ends with one newline."
                            .to_string(),
                    ),
                    types: Some(vec!["text".to_string()]),
                    stages: Some(vec![Stage::PreCommit, Stage::PrePush, Stage::Manual]),
                    ..Default::default()
                },
            },
            BuiltinHooks::FixByteOrderMarker => ManifestHook {
                id: "fix-byte-order-marker".to_string(),
                name: "fix utf-8 byte order marker".to_string(),
                language: Language::Python,
                entry: "fix-byte-order-marker".to_string(),
                options: HookOptions {
                    description: Some("removes utf-8 byte order marker.".to_string()),
                    types: Some(vec!["text".to_string()]),
                    ..Default::default()
                },
            },
            BuiltinHooks::CheckJson => ManifestHook {
                id: "check-json".to_string(),
                name: "check json".to_string(),
                language: Language::Python,
                entry: "check-json".to_string(),
                options: HookOptions {
                    description: Some("checks json files for parseable syntax.".to_string()),
                    types: Some(vec!["json".to_string()]),
                    ..Default::default()
                },
            },
            BuiltinHooks::CheckSymlinks => ManifestHook {
                id: "check-symlinks".to_string(),
                name: "check for broken symlinks".to_string(),
                language: Language::Python,
                entry: "check-symlinks".to_string(),
                options: HookOptions {
                    description: Some(
                        "checks for symlinks which do not point to anything.".to_string(),
                    ),
                    types: Some(vec!["symlink".to_string()]),
                    ..Default::default()
                },
            },
            BuiltinHooks::CheckMergeConflict => ManifestHook {
                id: "check-merge-conflict".to_string(),
                name: "check for merge conflicts".to_string(),
                language: Language::Python,
                entry: "check-merge-conflict".to_string(),
                options: HookOptions {
                    description: Some(
                        "checks for files that contain merge conflict strings.".to_string(),
                    ),
                    types: Some(vec!["text".to_string()]),
                    ..Default::default()
                },
            },
            BuiltinHooks::CheckToml => ManifestHook {
                id: "check-toml".to_string(),
                name: "check toml".to_string(),
                language: Language::Python,
                entry: "check-toml".to_string(),
                options: HookOptions {
                    description: Some("checks toml files for parseable syntax.".to_string()),
                    types: Some(vec!["toml".to_string()]),
                    ..Default::default()
                },
            },
            BuiltinHooks::CheckXml => ManifestHook {
                id: "check-xml".to_string(),
                name: "check xml".to_string(),
                language: Language::Python,
                entry: "check-xml".to_string(),
                options: HookOptions {
                    description: Some("checks xml files for parseable syntax.".to_string()),
                    types: Some(vec!["xml".to_string()]),
                    ..Default::default()
                },
            },
            BuiltinHooks::CheckYaml => ManifestHook {
                id: "check-yaml".to_string(),
                name: "check yaml".to_string(),
                language: Language::Python,
                entry: "check-yaml".to_string(),
                options: HookOptions {
                    description: Some("checks yaml files for parseable syntax.".to_string()),
                    types: Some(vec!["yaml".to_string()]),
                    ..Default::default()
                },
            },
            BuiltinHooks::MixedLineEnding => ManifestHook {
                id: "mixed-line-ending".to_string(),
                name: "mixed line ending".to_string(),
                language: Language::Python,
                entry: "mixed-line-ending".to_string(),
                options: HookOptions {
                    description: Some("replaces or checks mixed line ending.".to_string()),
                    types: Some(vec!["text".to_string()]),
                    ..Default::default()
                },
            },
            BuiltinHooks::DetectPrivateKey => ManifestHook {
                id: "detect-private-key".to_string(),
                name: "detect private key".to_string(),
                language: Language::Python,
                entry: "detect-private-key".to_string(),
                options: HookOptions {
                    description: Some("detects the presence of private keys.".to_string()),
                    types: Some(vec!["text".to_string()]),
                    ..Default::default()
                },
            },
            BuiltinHooks::NoCommitToBranch => ManifestHook {
                id: "no-commit-to-branch".to_string(),
                name: "don't commit to branch".to_string(),
                language: Language::Python,
                entry: "no-commit-to-branch".to_string(),
                options: HookOptions {
                    pass_filenames: Some(false),
                    always_run: Some(true),
                    ..Default::default()
                },
            },
            BuiltinHooks::CheckExecutablesHaveShebangs => ManifestHook {
                id: "check-executables-have-shebangs".to_string(),
                name: "check that executables have shebangs".to_string(),
                language: Language::Python,
                entry: "check-executables-have-shebangs".to_string(),
                options: HookOptions {
                    description: Some(
                        "ensures that (non-binary) executables have a shebang.".to_string(),
                    ),
                    types: Some(vec!["text".to_string(), "executable".to_string()]),
                    stages: Some(vec![Stage::PreCommit, Stage::PrePush, Stage::Manual]),
                    ..Default::default()
                },
            },
        };

        Ok(BuiltinHook(hook))
    }
}
