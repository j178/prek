use crate::hook::{HookEffect, RepoUrl};

/// Only audited execution paths receive a guarantee; a checker can still have write effects.
pub(crate) fn remote_hook_effect(repo: RepoUrl<'_>, id: &str) -> HookEffect {
    if let Some(path) = repo.path_on("github.com") {
        match (path, id) {
            ("adrienverge/yamllint", "yamllint")
            | ("asottile/dead", "dead")
            | ("checkmake/checkmake", "checkmake" | "checkmake-system")
            | ("compilerla/conventional-pre-commit", "conventional-pre-commit")
            | ("jendrikseipp/vulture", "vulture")
            | ("Lucas-C/pre-commit-hooks", "forbid-crlf" | "forbid-tabs")
            | ("pre-commit/mirrors-csslint", "csslint")
            | ("shellcheck-py/shellcheck-py", "shellcheck") => HookEffect::ReadOnly,
            (
                "pre-commit/pre-commit-hooks",
                "check-added-large-files"
                | "check-ast"
                | "check-builtin-literals"
                | "check-case-conflict"
                | "check-docstring-first"
                | "check-executables-have-shebangs"
                | "check-illegal-windows-names"
                | "check-json"
                | "check-merge-conflict"
                | "check-shebang-scripts-are-executable"
                | "check-symlinks"
                | "check-toml"
                | "check-vcs-permalinks"
                | "check-xml"
                | "check-yaml"
                | "debug-statements"
                | "detect-aws-credentials"
                | "detect-private-key"
                | "forbid-new-submodules"
                | "forbid-submodules"
                | "name-tests-test"
                | "no-commit-to-branch",
            ) => HookEffect::ReadOnly,
            (
                "pre-commit/pygrep-hooks",
                "python-check-blanket-noqa"
                | "python-check-blanket-type-ignore"
                | "python-check-mock-methods"
                | "python-no-eval"
                | "python-no-log-warn"
                | "python-use-type-annotations"
                | "rst-backticks"
                | "rst-directive-colons"
                | "rst-inline-touching-normal"
                | "text-unicode-replacement-char",
            ) => HookEffect::ReadOnly,
            // git status can refresh the shared index.
            ("pre-commit/pre-commit-hooks", "destroyed-symlinks") => HookEffect::WorkspaceReadOnly,
            // The Docker lifecycle has effects beyond shellcheck's workspace reads.
            ("koalaman/shellcheck-precommit", "shellcheck") => HookEffect::WorkspaceReadOnly,
            // Output paths, executable configuration, and custom reporters or validators
            // prevent a workspace-read-only guarantee, even for otherwise read-only checks.
            _ => HookEffect::Unknown,
        }
    } else if let Some(path) = repo.path_on("gitlab.com") {
        match (path, id) {
            ("bmares/check-json5", "check-json5") => HookEffect::ReadOnly,
            _ => HookEffect::Unknown,
        }
    } else {
        HookEffect::Unknown
    }
}
