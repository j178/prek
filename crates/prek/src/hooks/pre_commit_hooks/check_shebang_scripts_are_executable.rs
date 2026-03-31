use std::fmt::Write as _;
use std::path::Path;

use owo_colors::OwoColorize;

use crate::hook::Hook;
use crate::hooks::pre_commit_hooks::shebangs::{
    file_has_shebang, git_index_stage_output, matching_git_index_paths_by_executable_bit,
};
use crate::hooks::run_concurrent_file_checks;
use crate::run::CONCURRENCY;
use rustc_hash::FxHashSet;

pub(crate) async fn check_shebang_scripts_are_executable(
    hook: &Hook,
    filenames: &[&Path],
) -> Result<(i32, Vec<u8>), anyhow::Error> {
    let file_base = hook.project().relative_path();
    let stdout = git_index_stage_output(file_base).await?;
    let filenames: FxHashSet<_> = filenames.iter().copied().collect();
    let entries = matching_git_index_paths_by_executable_bit(&stdout, file_base, &filenames, false);

    run_concurrent_file_checks(entries, *CONCURRENCY, |file| async move {
        let file_path = file_base.join(file);
        if file_has_shebang(&file_path).await? {
            Ok((1, build_non_executable_shebang_warning(file)?.into_bytes()))
        } else {
            Ok((0, Vec::new()))
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
