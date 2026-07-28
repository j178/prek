use std::fmt::Write as _;
use std::path::Path;

use owo_colors::OwoColorize;

use crate::hook::Hook;
use crate::hooks::HookOutput;
use crate::hooks::pre_commit_hooks::shebangs::{
    file_has_shebang, git_index_stage_output, matching_git_index_paths_by_executable_bit,
};
use crate::hooks::pre_commit_hooks::{FilenamesArgs, hook_filenames, parse_hook_args};
use crate::hooks::run_concurrent_file_checks;
use crate::run::INTERNAL_CONCURRENCY;
use rustc_hash::FxHashSet;

/// Runs the `check-shebang-scripts-are-executable` hook.
pub(crate) async fn run(hook: &Hook, filenames: &[&Path]) -> Result<HookOutput, anyhow::Error> {
    let args: FilenamesArgs = parse_hook_args(hook)?;
    let filenames = hook_filenames(&args.filenames, filenames).collect::<Vec<_>>();
    if filenames.is_empty() {
        return Ok(HookOutput::unchanged(0, Vec::new()));
    }

    let file_base = hook.project().relative_path();
    let stdout = git_index_stage_output(file_base).await?;
    let filenames: FxHashSet<_> = filenames.into_iter().collect();
    let entries = matching_git_index_paths_by_executable_bit(&stdout, file_base, &filenames, false);

    run_concurrent_file_checks(entries, *INTERNAL_CONCURRENCY, |file| async move {
        let file_path = file_base.join(file);
        if file_has_shebang(&file_path).await? {
            Ok(HookOutput::unchanged(
                1,
                build_non_executable_shebang_warning(file)?.into_bytes(),
            ))
        } else {
            Ok(HookOutput::unchanged(0, Vec::new()))
        }
    })
    .await
}

fn build_non_executable_shebang_warning(path: &Path) -> Result<String, std::fmt::Error> {
    let path_str = path.display();
    let mut warning = String::new();
    writeln!(
        warning,
        "{}",
        format!(
            "{} has a shebang but is not marked executable!",
            path_str.yellow()
        )
        .bold()
    )?;
    writeln!(
        warning,
        "{}",
        format!("  If it is supposed to be executable, try: 'chmod +x {path_str}'").dimmed()
    )?;
    writeln!(
        warning,
        "{}",
        format!("  If on Windows, you may also need to: 'git add --chmod=+x {path_str}'").dimmed()
    )?;
    writeln!(
        warning,
        "{}",
        "  If it is not supposed to be executable, double-check its shebang is wanted.".dimmed()
    )?;
    Ok(warning)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_executable_warning_mentions_chmod_and_git_add() {
        let warning = build_non_executable_shebang_warning(Path::new("script.sh")).unwrap();

        assert!(warning.contains("chmod +x script.sh"));
        assert!(warning.contains("git add --chmod=+x script.sh"));
    }
}
