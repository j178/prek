use crate::cli::ExitStatus;

static SAMPLE_CONFIG: &str = "\
# See https://pre-commit.com for more information
# See https://pre-commit.com/hooks.html for more hooks
repos:
-   repo: https://github.com/pre-commit/pre-commit-hooks
    rev: v5.0.0
    hooks:
    -   id: trailing-whitespace
    -   id: end-of-file-fixer
    -   id: check-yaml
    -   id: check-added-large-files
";

#[allow(clippy::print_stdout)]
pub(crate) fn sample_config() -> ExitStatus {
    print!("{SAMPLE_CONFIG}");
    ExitStatus::Success
}
