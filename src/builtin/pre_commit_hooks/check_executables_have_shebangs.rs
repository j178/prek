use std::path::{Path, PathBuf};

use futures::StreamExt;
use owo_colors::OwoColorize;
use tokio::io::AsyncReadExt;

use crate::git;
use crate::hook::Hook;
use crate::run::CONCURRENCY;

const EXECUTABLE_VALUES: &[char] = &['1', '3', '5', '7'];

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
    let file_paths: Vec<_> = filenames.iter().map(|p| file_base.join(p)).collect();

    // If on win32 use git to check executable bit, else use os level check
    let (code, output) = if tracks_executable_bit {
        os_check_shebangs(&file_paths).await?
    } else {
        git_check_shebangs(&file_paths).await?
    };

    Ok((code, output))
}

async fn os_check_shebangs(paths: &Vec<PathBuf>) -> Result<(i32, Vec<u8>), anyhow::Error> {
    let mut tasks = futures::stream::iter(paths)
        .map(|file| async move {
            let has_shebang = file_has_shebang(file).await?;
            if has_shebang {
                anyhow::Ok((0, Vec::new()))
            } else {
                let msg = print_shebang_warning(file);
                Ok((1, msg.into_bytes()))
            }
        })
        .buffered(*CONCURRENCY);

    let mut code = 0;
    let mut output = Vec::new();
    while let Some(result) = tasks.next().await {
        let (c, o) = result?;
        code |= c;
        output.extend(o);
    }

    Ok((code, output))
}

fn print_shebang_warning(path: &Path) -> String {
    let path_str = path.display();

    format!(
        "{}\n\
         {}\n\
         {}\n\
         {}\n",
        format!(
            "{} marked executable but has no (or invalid) shebang!",
            path_str.yellow()
        )
        .bold(),
        format!("  If it isn't supposed to be executable, try: 'chmod -x {path_str}'").dimmed(),
        format!("  If on Windows, you may also need to: 'git add --chmod=-x {path_str}'").dimmed(),
        "  If it is supposed to be executable, double-check its shebang.".dimmed(),
    )
}

async fn git_check_shebangs(paths: &Vec<PathBuf>) -> Result<(i32, Vec<u8>), anyhow::Error> {
    let output = git::git_cmd("git ls-files")?
        .arg("ls-files")
        .arg("-z")
        // Show staged contents' mode bits, object name and stage number in the output.
        .arg("--stage")
        .arg("--")
        .args(paths)
        .check(true)
        .output()
        .await?;

    let entries = output.stdout.split(|&b| b == b'\0').filter_map(|entry| {
        let entry = str::from_utf8(entry).ok()?;
        if entry.is_empty() {
            return None;
        }
        let mut parts = entry.split('\t');
        let metadata = parts.next()?;
        let file_name = parts.next()?;
        let mode = metadata.split_whitespace().next().unwrap_or("");
        let is_executable = mode
            .chars()
            .rev()
            .take(3)
            .any(|c| EXECUTABLE_VALUES.contains(&c));
        Some((file_name.to_string(), is_executable))
    });

    let mut tasks = futures::stream::iter(entries)
        .map(|(file_name, is_executable)| async move {
            if is_executable {
                let has_shebang = file_has_shebang(Path::new(&file_name)).await?;
                if has_shebang {
                    anyhow::Ok((0, Vec::new()))
                } else {
                    let msg = print_shebang_warning(Path::new(&file_name));
                    Ok((1, msg.into_bytes()))
                }
            } else {
                Ok((0, Vec::new()))
            }
        })
        .buffered(*CONCURRENCY);

    let mut code = 0;
    let mut output = Vec::new();

    while let Some(result) = tasks.next().await {
        let (c, o) = result?;
        code |= c;
        output.extend(o);
    }

    Ok((code, output))
}

/// Check first 2 bytes for shebang (#!)
async fn file_has_shebang(path: &Path) -> Result<bool, anyhow::Error> {
    let mut file = fs_err::tokio::File::open(path).await?;
    let mut buf = [0u8; 2];
    let n = file.read(&mut buf).await?;
    Ok(n >= 2 && buf[0] == b'#' && buf[1] == b'!')
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    // file_has_shebang tests
    #[tokio::test]
    async fn test_file_with_shebang() -> Result<(), anyhow::Error> {
        let dir = tempdir()?;
        let file_path = dir.path().join("script.sh");
        tokio::fs::write(&file_path, b"#!/bin/bash\necho Hello World\n").await?;

        assert!(file_has_shebang(&file_path).await?);
        Ok(())
    }

    #[tokio::test]
    async fn test_file_without_shebang() -> Result<(), anyhow::Error> {
        let dir = tempdir()?;
        let file_path = dir.path().join("script.sh");
        tokio::fs::write(&file_path, b"echo Hello World\n").await?;

        assert!(!file_has_shebang(&file_path).await?);
        Ok(())
    }

    #[tokio::test]
    async fn test_empty_file() -> Result<(), anyhow::Error> {
        let dir = tempdir()?;
        let file_path = dir.path().join("empty.sh");
        tokio::fs::write(&file_path, b"").await?;

        assert!(!file_has_shebang(&file_path).await?);
        Ok(())
    }

    #[tokio::test]
    async fn test_file_with_partial_shebang() -> Result<(), anyhow::Error> {
        let dir = tempdir()?;
        let file_path = dir.path().join("partial.sh");
        tokio::fs::write(&file_path, b"#\n").await?;
        assert!(!file_has_shebang(&file_path).await?);
        Ok(())
    }

    #[tokio::test]
    async fn test_file_with_shebang_and_spaces() -> Result<(), anyhow::Error> {
        let dir = tempdir()?;
        let file_path = dir.path().join("spaces.sh");
        tokio::fs::write(&file_path, b"#! /bin/bash\necho Test\n").await?;
        assert!(file_has_shebang(&file_path).await?);
        Ok(())
    }

    #[tokio::test]
    async fn test_file_with_non_shebang_start() -> Result<(), anyhow::Error> {
        let dir = tempdir()?;
        let file_path = dir.path().join("nonshebang.sh");
        tokio::fs::write(&file_path, b"##!/bin/bash\n").await?;
        assert!(!file_has_shebang(&file_path).await?);
        Ok(())
    }

    // integration tests for os_check_shebangs
    #[tokio::test]
    async fn test_os_check_shebangs_with_shebang() -> Result<(), anyhow::Error> {
        let dir = tempdir()?;
        let file_path = dir.path().join("with_shebang.sh");
        tokio::fs::write(&file_path, b"#!/bin/bash\necho ok\n").await?;
        let files = vec![file_path.clone()];
        let (code, output) = os_check_shebangs(&files).await?;
        assert_eq!(code, 0);
        assert!(output.is_empty());
        Ok(())
    }

    // integration tests for os_check_shebangs
    #[tokio::test]
    async fn test_os_check_shebangs_without_shebang() -> Result<(), anyhow::Error> {
        let dir = tempdir()?;
        let file_path = dir.path().join("without_shebang.sh");
        tokio::fs::write(&file_path, b"echo ok\n").await?;
        let files = vec![file_path.clone()];
        let (code, output) = os_check_shebangs(&files).await?;
        assert_eq!(code, 1);
        assert!(
            String::from_utf8_lossy(&output)
                .contains("marked executable but has no (or invalid) shebang!")
        );
        Ok(())
    }
}
