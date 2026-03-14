use std::path::Path;

use futures::StreamExt;
use owo_colors::OwoColorize;
use tokio::io::AsyncReadExt;

use crate::hook::Hook;
use crate::hooks::run_concurrent_file_checks;
use crate::repo;
use crate::run::CONCURRENCY;

pub(crate) async fn check_executables_have_shebangs(
    hook: &Hook,
    filenames: &[&Path],
) -> Result<(i32, Vec<u8>), anyhow::Error> {
    let tracks_executable_bit = repo::tracks_executable_bit().await?;
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
            let msg = print_shebang_warning(file);
            Ok((1, msg.into_bytes()))
        }
    })
    .await
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

async fn git_check_shebangs(
    file_base: &Path,
    filenames: &[&Path],
) -> Result<(i32, Vec<u8>), anyhow::Error> {
    let executable_files = repo::executable_files(file_base, filenames).await?;

    let mut tasks = futures::stream::iter(executable_files)
        .map(async |file_name| {
            let full_path = file_base.join(&file_name);
            let has_shebang = file_has_shebang(&full_path).await?;
            if has_shebang {
                anyhow::Ok((0, Vec::new()))
            } else {
                let msg = print_shebang_warning(&file_name);
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
    use tempfile::NamedTempFile;

    #[tokio::test]
    async fn test_file_with_shebang() -> Result<(), anyhow::Error> {
        let file = NamedTempFile::new()?;
        tokio::fs::write(file.path(), b"#!/bin/bash\necho Hello World\n").await?;

        assert!(file_has_shebang(file.path()).await?);
        Ok(())
    }

    #[tokio::test]
    async fn test_file_without_shebang() -> Result<(), anyhow::Error> {
        let file = NamedTempFile::new()?;
        tokio::fs::write(file.path(), b"echo Hello World\n").await?;

        assert!(!file_has_shebang(file.path()).await?);
        Ok(())
    }

    #[tokio::test]
    async fn test_empty_file() -> Result<(), anyhow::Error> {
        let file = NamedTempFile::new()?;
        tokio::fs::write(file.path(), b"").await?;

        assert!(!file_has_shebang(file.path()).await?);
        Ok(())
    }

    #[tokio::test]
    async fn test_file_with_partial_shebang() -> Result<(), anyhow::Error> {
        let file = NamedTempFile::new()?;
        tokio::fs::write(file.path(), b"#\n").await?;
        assert!(!file_has_shebang(file.path()).await?);
        Ok(())
    }

    #[tokio::test]
    async fn test_file_with_shebang_and_spaces() -> Result<(), anyhow::Error> {
        let file = NamedTempFile::new()?;
        tokio::fs::write(file.path(), b"#! /bin/bash\necho Test\n").await?;
        assert!(file_has_shebang(file.path()).await?);
        Ok(())
    }

    #[tokio::test]
    async fn test_file_with_non_shebang_start() -> Result<(), anyhow::Error> {
        let file = NamedTempFile::new()?;
        tokio::fs::write(file.path(), b"##!/bin/bash\n").await?;
        assert!(!file_has_shebang(file.path()).await?);
        Ok(())
    }

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
