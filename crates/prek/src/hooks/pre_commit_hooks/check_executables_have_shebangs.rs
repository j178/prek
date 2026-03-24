use std::fmt::Write as _;
use std::path::Path;

use owo_colors::OwoColorize;

use crate::git;
use crate::hook::Hook;
use crate::hooks::pre_commit_hooks::shebangs::{
    file_has_shebang, git_index_stage_output, matching_git_index_paths_by_executable_bit,
};
use crate::hooks::run_concurrent_file_checks;
use crate::run::CONCURRENCY;
use rustc_hash::FxHashSet;

pub(crate) async fn check_executables_have_shebangs(
    hook: &Hook,
    filenames: &[&Path],
) -> Result<(i32, Vec<u8>), anyhow::Error> {
    let stdout = git::git_cmd("get file file mode")?
        .arg("config")
        .arg("core.fileMode")
        .check(true)
        .output()
        .await?
        .stdout;

    let tracks_executable_bit = std::str::from_utf8(&stdout)?.trim() != "false";
    let file_base = hook.project().relative_path();

    let (code, output) = if tracks_executable_bit {
        // core.fileMode=true means the platform honors the executable bit, so trust the FS metadata.
        // The `executables-have-shebangs` hook already restricts inputs to executable text files (`types: [text, executable]`).
        os_check_shebangs(file_base, filenames).await?
    } else {
        // If on win32 use git to check executable bit
        git_check_shebangs(file_base, filenames).await?
    };

    Ok((code, output))
}

async fn os_check_shebangs(
    file_base: &Path,
    paths: &[&Path],
) -> Result<(i32, Vec<u8>), anyhow::Error> {
    run_concurrent_file_checks(paths.iter().copied(), *CONCURRENCY, |file| async move {
        let file_path = file_base.join(file);
        let has_shebang = file_has_shebang(&file_path).await?;
        if has_shebang {
            anyhow::Ok((0, Vec::new()))
        } else {
            let msg = build_missing_shebang_warning(file)?;
            Ok((1, msg.into_bytes()))
        }
    })
    .await
}

fn build_missing_shebang_warning(path: &Path) -> Result<String, std::fmt::Error> {
    let path_str = path.display();
    let mut warning = String::new();
    writeln!(
        warning,
        "{}",
        format!(
            "{} marked executable but has no (or invalid) shebang!",
            path_str.yellow()
        )
        .bold()
    )?;
    writeln!(
        warning,
        "{}",
        format!("  If it isn't supposed to be executable, try: 'chmod -x {path_str}'").dimmed()
    )?;
    writeln!(
        warning,
        "{}",
        format!("  If on Windows, you may also need to: 'git add --chmod=-x {path_str}'").dimmed()
    )?;
    writeln!(
        warning,
        "{}",
        "  If it is supposed to be executable, double-check its shebang.".dimmed()
    )?;
    Ok(warning)
}

async fn git_check_shebangs(
    file_base: &Path,
    filenames: &[&Path],
) -> Result<(i32, Vec<u8>), anyhow::Error> {
    let stdout = git_index_stage_output(file_base).await?;
    let filenames: FxHashSet<_> = filenames.iter().copied().collect();
    let entries = matching_git_index_paths_by_executable_bit(&stdout, file_base, &filenames, true);

    run_concurrent_file_checks(entries, *CONCURRENCY, |file| async move {
        let file_path = file_base.join(file);
        if file_has_shebang(&file_path).await? {
            Ok((0, Vec::new()))
        } else {
            Ok((1, build_missing_shebang_warning(file)?.into_bytes()))
        }
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    #[tokio::test]
    async fn test_os_check_shebangs_with_shebang() -> Result<(), anyhow::Error> {
        let file = NamedTempFile::new()?;
        tokio::fs::write(file.path(), b"#!/bin/bash\necho ok\n").await?;
        let files = vec![file.path()];
        let (code, output) = os_check_shebangs(Path::new(""), &files).await?;
        assert_eq!(code, 0);
        assert!(output.is_empty());

        Ok(())
    }

    #[tokio::test]
    async fn test_os_check_shebangs_without_shebang() -> Result<(), anyhow::Error> {
        let file = NamedTempFile::new()?;
        tokio::fs::write(file.path(), b"echo ok\n").await?;
        let files = vec![file.path()];
        let (code, output) = os_check_shebangs(Path::new(""), &files).await?;
        assert_eq!(code, 1);
        assert!(
            String::from_utf8_lossy(&output)
                .contains("marked executable but has no (or invalid) shebang!")
        );
        Ok(())
    }
}
