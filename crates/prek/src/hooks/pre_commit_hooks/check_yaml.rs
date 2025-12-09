use std::path::Path;

use anyhow::Result;
use clap::Parser;
use futures::StreamExt;
use serde::Deserialize;

use crate::hook::Hook;
use crate::run::CONCURRENCY;

#[derive(Parser)]
#[command(disable_help_subcommand = true)]
#[command(disable_version_flag = true)]
#[command(disable_help_flag = true)]
struct Args {
    #[arg(long, short = 'm', alias = "multi")]
    allow_multiple_documents: bool,
    // `--unsafe` flag is not supported yet.
    // #[arg(long)]
    // r#unsafe: bool,
}

pub(crate) async fn check_yaml(hook: &Hook, filenames: &[&Path]) -> Result<(i32, Vec<u8>)> {
    let args = Args::try_parse_from(hook.entry.resolve(None)?.iter().chain(&hook.args))?;

    let mut tasks = futures::stream::iter(filenames)
        .map(async |filename| {
            check_file(
                hook.project().relative_path(),
                filename,
                args.allow_multiple_documents,
            )
            .await
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

async fn check_file(
    file_base: &Path,
    filename: &Path,
    allow_multi_docs: bool,
) -> Result<(i32, Vec<u8>)> {
    let content = fs_err::tokio::read(file_base.join(filename)).await?;
    if content.is_empty() {
        return Ok((0, Vec::new()));
    }

    let deserializer = serde_yaml::Deserializer::from_slice(&content);
    if allow_multi_docs {
        for doc in deserializer {
            if let Err(e) = serde_yaml::Value::deserialize(doc) {
                let error_message =
                    format!("{}: Failed to yaml decode ({e})\n", filename.display());
                return Ok((1, error_message.into_bytes()));
            }
        }
        Ok((0, Vec::new()))
    } else {
        match serde_yaml::from_slice::<serde_yaml::Value>(&content) {
            Ok(_) => Ok((0, Vec::new())),
            Err(e) => {
                let error_message =
                    format!("{}: Failed to yaml decode ({e})\n", filename.display());
                Ok((1, error_message.into_bytes()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::tempdir;

    async fn create_test_file(
        dir: &tempfile::TempDir,
        name: &str,
        content: &[u8],
    ) -> Result<PathBuf> {
        let file_path = dir.path().join(name);
        fs_err::tokio::write(&file_path, content).await?;
        Ok(file_path)
    }

    #[tokio::test]
    async fn test_valid_yaml() -> Result<()> {
        let dir = tempdir()?;
        let content = br"key1: value1
key2: value2
";
        let file_path = create_test_file(&dir, "valid.yaml", content).await?;
        let (code, output) = check_file(Path::new(""), &file_path, false).await?;
        assert_eq!(code, 0);
        assert!(output.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn test_invalid_yaml() -> Result<()> {
        let dir = tempdir()?;
        let content = br"key1: value1
key2: value2: another_value
";
        let file_path = create_test_file(&dir, "invalid.yaml", content).await?;
        let (code, output) = check_file(Path::new(""), &file_path, false).await?;
        assert_eq!(code, 1);
        assert!(!output.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn test_duplicate_keys() -> Result<()> {
        let dir = tempdir()?;
        let content = br"key1: value1
key1: value2
";
        let file_path = create_test_file(&dir, "duplicate.yaml", content).await?;
        let (code, output) = check_file(Path::new(""), &file_path, false).await?;
        assert_eq!(code, 1);
        assert!(!output.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn test_empty_yaml() -> Result<()> {
        let dir = tempdir()?;
        let content = b"";
        let file_path = create_test_file(&dir, "empty.yaml", content).await?;
        let (code, output) = check_file(Path::new(""), &file_path, false).await?;
        assert_eq!(code, 0);
        assert!(output.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn test_multiple_documents() -> Result<()> {
        let dir = tempdir()?;
        let content = b"\
---
key1: value1
---
key2: value2
";
        let file_path = create_test_file(&dir, "multi.yaml", content).await?;

        let (code, output) = check_file(Path::new(""), &file_path, false).await?;
        assert_eq!(code, 1);
        assert!(!output.is_empty());

        let (code, output) = check_file(Path::new(""), &file_path, true).await?;
        assert_eq!(code, 0);
        assert!(output.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn test_yaml_with_binary_scalar() -> Result<()> {
        let dir = tempdir()?;
        let content = b"\
response:
  body:
    string: !!binary |
      H4sIAAAAAAAAA4xTPW/bMBDd9SsON9uFJaeJ4y0oujRIEXQpisiQaOokM6VIgjzFSQ3/94KSYzmt
      A2TRwPfBd/eoXQKAqsIloNwIlq3T0y/rF6JfbXYT2m3rvan+NLfXt/zj2/f5NsVJVNj1I0l+VX2S
      tnWaWFkzwNKTYIqu6dXlPL28mmeLHmhtRTrKGsfTCzvNZtnFNE2n2ewg3FglKeASHhIAgF3/jRFN
      Rc+4hNnk9aSlEERDuDySANBbHU9QhKACC8M4GUFpDZPpU5dl+Risyc0uNwA5smJNOS4hxxu4Jx8c
      SVZPBNbA12enhRFxugC2hjurSXZaeLj3VCkZAbiLg4UcJ4Of6HhjfYiODzn+JK3FVjATEIPQOa4O
      vMqqyDGd1rnZ56Ysy9PEnuouCH1gnADCGMtDpHjF6oDsj9vRtnHersM/UqyVUWFTeBLBmriJwNZh
      j+4TgFXfQvdmsei8bR0XbH9Tf91iPtjhWPsIzq8PIFsWejxPs2xyxq6oiIXS4aRGlEJuqBqlY+ei
      q5Q9AZKTof9Pc857GFyZ5iP2IyAlOaaqcMfGz9E8xb/iPdpxyX1gDOSflKSCFflYREW16PTwYDG8
      BKa2qJVpyDuvhldbu0LOFtnicypnC0z2yV8AAAD//wMALvIkjL4DAAA=
";
        let file_path = create_test_file(&dir, "binary.yaml", content).await?;
        let (code, output) = check_file(Path::new(""), &file_path, false).await?;
        assert_eq!(code, 0);
        assert!(output.is_empty());
        Ok(())
    }
}
