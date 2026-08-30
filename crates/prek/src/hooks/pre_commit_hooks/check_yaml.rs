use std::path::{Path, PathBuf};

use anyhow::Result;
use clap::Parser;
use serde::de::IgnoredAny;

use crate::hook::Hook;
use crate::hooks::HookOutput;
use crate::hooks::pre_commit_hooks::{hook_filenames, parse_hook_args};
use crate::hooks::run_concurrent_file_checks;
use crate::run::INTERNAL_CONCURRENCY;

#[derive(Parser)]
#[command(disable_help_subcommand = true)]
#[command(disable_version_flag = true)]
#[command(disable_help_flag = true)]
pub(crate) struct Args {
    /// Allow multiple YAML documents.
    #[arg(long, short = 'm', visible_alias = "multi")]
    allow_multiple_documents: bool,
    /// Parse YAML syntax without loading it. Implies `--allow-multiple-documents`.
    #[arg(long)]
    r#unsafe: bool,
    #[arg(value_name = "FILENAMES")]
    filenames: Vec<PathBuf>,
}

#[derive(Clone, Copy)]
enum CheckMode {
    Load { multiple: bool },
    SyntaxOnly,
}

/// Runs the `check-yaml` hook.
pub(crate) async fn run(hook: &Hook, filenames: &[&Path]) -> Result<HookOutput> {
    let args: Args = parse_hook_args(hook)?;
    let mode = if args.r#unsafe {
        CheckMode::SyntaxOnly
    } else {
        CheckMode::Load {
            multiple: args.allow_multiple_documents,
        }
    };

    run_concurrent_file_checks(
        hook_filenames(&args.filenames, filenames),
        *INTERNAL_CONCURRENCY,
        |filename| check_file(hook.project().relative_path(), filename, mode),
    )
    .await
}

async fn check_file(file_base: &Path, filename: &Path, mode: CheckMode) -> Result<HookOutput> {
    let content = fs_err::tokio::read(file_base.join(filename)).await?;
    if content.is_empty() {
        return Ok(HookOutput::unchanged(0, Vec::new()));
    }

    let output = match mode {
        CheckMode::Load { multiple } => check_loaded(filename, &content, multiple),
        CheckMode::SyntaxOnly => check_syntax(filename, &content),
    };
    Ok(output)
}

fn check_loaded(filename: &Path, content: &[u8], allow_multi_docs: bool) -> HookOutput {
    let options = serde_saphyr::options! {
        budget: serde_saphyr::budget! {
            // `check-yaml` is a syntax/structure validator, not a service parsing
            // untrusted YAML at runtime. Keep the absolute caps, but allow
            // high-reuse anchors that are common in compose-style files. See #1838.
            enforce_alias_anchor_ratio: false,
        },
        // Comments do not affect syntax or structure validation. See #2501.
        emit_comments: false,
        // Do not require `!!binary` scalars to decode as UTF-8. See #1102.
        ignore_binary_tag_for_string: true,
        // Match ruamel.yaml's safe loader by rejecting tags without constructors. See #2604.
        reject_unsupported_tags: true,
        // The scalar values are discarded, so only validate whether they are
        // legal YAML, not whether an untyped data model can represent them. See #2544.
        reject_non_finite_typeless_float: false,
    };
    let result = if allow_multi_docs {
        serde_saphyr::from_slice_multiple_with_options::<IgnoredAny>(content, options).map(|_| ())
    } else {
        serde_saphyr::from_slice_with_options::<IgnoredAny>(content, options).map(|_| ())
    };
    match result {
        Ok(()) => HookOutput::unchanged(0, Vec::new()),
        Err(e) => {
            let err = e.render_with_formatter(&serde_saphyr::UserMessageFormatter);
            let error_message = format!("{}: Failed to yaml decode ({err})\n", filename.display());
            HookOutput::unchanged(1, error_message.into_bytes())
        }
    }
}

fn check_syntax(filename: &Path, content: &[u8]) -> HookOutput {
    let content = match std::str::from_utf8(content) {
        Ok(content) => content,
        Err(error) => {
            let error_message =
                format!("{}: Failed to decode UTF-8 ({error})\n", filename.display());
            return HookOutput::unchanged(1, error_message.into_bytes());
        }
    };

    let options = granit_parser::options! {
        // Comments do not affect syntax validation. See #2501.
        emit_comments: false,
    };

    // TODO: `granit-parser` resolves aliases while producing parser events, so this is stricter
    // than upstream's syntax-only mode for aliases that reference an undefined anchor.
    for event in granit_parser::Parser::with_options(granit_parser::StrInput::new(content), options)
    {
        if let Err(error) = event {
            let error_message = format!("{}: Failed to yaml parse ({error})\n", filename.display());
            return HookOutput::unchanged(1, error_message.into_bytes());
        }
    }

    HookOutput::unchanged(0, Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use std::fmt::Write;
    use std::path::PathBuf;
    use tempfile::tempdir;

    const LOAD_SINGLE_DOCUMENT: CheckMode = CheckMode::Load { multiple: false };
    const LOAD_MULTIPLE_DOCUMENTS: CheckMode = CheckMode::Load { multiple: true };

    async fn create_test_file(
        dir: &tempfile::TempDir,
        name: &str,
        content: &[u8],
    ) -> Result<PathBuf> {
        let file_path = dir.path().join(name);
        fs_err::tokio::write(&file_path, content).await?;
        Ok(file_path)
    }

    #[test]
    fn test_unsafe_argument() -> Result<()> {
        let args = Args::try_parse_from(["check-yaml", "--unsafe", "config.yaml"])?;
        assert!(args.r#unsafe);
        assert_eq!(args.filenames, [PathBuf::from("config.yaml")]);
        Ok(())
    }

    #[tokio::test]
    async fn test_valid_yaml() -> Result<()> {
        let dir = tempdir()?;
        let content = br"key1: value1
key2: value2
";
        let file_path = create_test_file(&dir, "valid.yaml", content).await?;
        let result = check_file(Path::new(""), &file_path, LOAD_SINGLE_DOCUMENT).await?;
        assert_eq!(result.exit_status, 0);
        assert!(result.output.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn test_yaml_with_non_finite_floats() -> Result<()> {
        let dir = tempdir()?;
        let content = br"nan: .nan
positive_infinity: .inf
negative_infinity: -.inf
";
        let file_path = create_test_file(&dir, "non-finite.yaml", content).await?;
        let result = check_file(Path::new(""), &file_path, LOAD_SINGLE_DOCUMENT).await?;
        assert_eq!(
            result.exit_status,
            0,
            "{}",
            String::from_utf8_lossy(&result.output)
        );
        assert!(result.output.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn test_invalid_yaml() -> Result<()> {
        let dir = tempdir()?;
        let content = br"key1: value1
key2: value2: another_value
";
        let file_path = create_test_file(&dir, "invalid.yaml", content).await?;
        let result = check_file(Path::new(""), &file_path, LOAD_SINGLE_DOCUMENT).await?;
        assert_eq!(result.exit_status, 1);
        assert!(!result.output.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn test_duplicate_keys() -> Result<()> {
        let dir = tempdir()?;
        let content = br"key1: value1
key1: value2
";
        let file_path = create_test_file(&dir, "duplicate.yaml", content).await?;
        let result = check_file(Path::new(""), &file_path, LOAD_SINGLE_DOCUMENT).await?;
        assert_eq!(result.exit_status, 1);
        assert!(!result.output.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn test_empty_yaml() -> Result<()> {
        let dir = tempdir()?;
        let content = b"";
        let file_path = create_test_file(&dir, "empty.yaml", content).await?;
        let result = check_file(Path::new(""), &file_path, LOAD_SINGLE_DOCUMENT).await?;
        assert_eq!(result.exit_status, 0);
        assert!(result.output.is_empty());
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

        let result = check_file(Path::new(""), &file_path, LOAD_SINGLE_DOCUMENT).await?;
        assert_eq!(result.exit_status, 1);
        assert!(!result.output.is_empty());

        let result = check_file(Path::new(""), &file_path, LOAD_MULTIPLE_DOCUMENTS).await?;
        assert_eq!(result.exit_status, 0);
        assert!(result.output.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn test_unsafe_allows_multiple_documents() -> Result<()> {
        let dir = tempdir()?;
        let content = b"---\nkey1: value1\n---\nkey2: value2\n";
        let file_path = create_test_file(&dir, "multi.yaml", content).await?;

        let result = check_file(Path::new(""), &file_path, CheckMode::SyntaxOnly).await?;
        assert_eq!(
            result.exit_status,
            0,
            "{}",
            String::from_utf8_lossy(&result.output)
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_unsafe_checks_later_documents() -> Result<()> {
        let dir = tempdir()?;
        let content = b"---\nkey: value\n---\n[";
        let file_path = create_test_file(&dir, "invalid-multi.yaml", content).await?;

        let result = check_file(Path::new(""), &file_path, CheckMode::SyntaxOnly).await?;
        assert_eq!(result.exit_status, 1);
        assert!(!result.output.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn test_unsafe_allows_values_rejected_while_loading() -> Result<()> {
        let dir = tempdir()?;
        let content = b"duplicate: first\nduplicate: second\ntarget:\n  <<: not-a-map\n";
        let file_path = create_test_file(&dir, "unsafe.yaml", content).await?;

        let result = check_file(Path::new(""), &file_path, CheckMode::SyntaxOnly).await?;
        assert_eq!(
            result.exit_status,
            0,
            "{}",
            String::from_utf8_lossy(&result.output)
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_unsafe_rejects_invalid_syntax() -> Result<()> {
        let dir = tempdir()?;
        let file_path = create_test_file(&dir, "invalid.yaml", b"[").await?;

        let result = check_file(Path::new(""), &file_path, CheckMode::SyntaxOnly).await?;
        assert_eq!(result.exit_status, 1);
        assert!(!result.output.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn test_unsafe_rejects_invalid_utf8() -> Result<()> {
        let dir = tempdir()?;
        let file_path = create_test_file(&dir, "invalid.yaml", b"key: \xff").await?;

        let result = check_file(Path::new(""), &file_path, CheckMode::SyntaxOnly).await?;
        assert_eq!(result.exit_status, 1);
        assert!(String::from_utf8_lossy(&result.output).contains("Failed to decode UTF-8"));
        Ok(())
    }

    #[test]
    fn test_unknown_yaml_tag_requires_unsafe() {
        let filename = Path::new("tagged.yaml");
        let content = b"foo: !reference [.bar, script]\n";

        let result = check_loaded(filename, content, false);
        assert_eq!(result.exit_status, 1);
        insta::assert_snapshot!(String::from_utf8_lossy(&result.output), @r#"
        tagged.yaml: Failed to yaml decode (error: line 1 column 17: unsupported tag `!reference`
         --> <input>:1:17
          |
        1 | foo: !reference [.bar, script]
          |                 ^ unsupported tag `!reference`)
        "#);

        let result = check_syntax(filename, content);
        assert_eq!((result.exit_status, result.output), (0, Vec::new()));
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
        let result = check_file(Path::new(""), &file_path, LOAD_SINGLE_DOCUMENT).await?;
        assert_eq!(result.exit_status, 0);
        assert!(result.output.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn test_yaml_with_many_aliases_and_few_anchors() -> Result<()> {
        let dir = tempdir()?;
        let mut content = indoc::formatdoc! {"
        defaults: &defaults
          image: alpine
        services:
        "};
        for index in 0..158 {
            let _ = write!(content, "  svc{index}:\n    <<: *defaults\n");
        }

        let file_path = create_test_file(&dir, "many-aliases.yaml", content.as_bytes()).await?;
        let result = check_file(Path::new(""), &file_path, LOAD_SINGLE_DOCUMENT).await?;
        assert_eq!(
            result.exit_status,
            0,
            "{}",
            String::from_utf8_lossy(&result.output)
        );
        assert!(result.output.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn test_yaml_with_many_consecutive_comments() -> Result<()> {
        let dir = tempdir()?;
        let mut content = "items:\n".to_string();
        for index in 0..100 {
            let _ = writeln!(content, "  # comment {index}");
        }
        content.push_str("  - value\n");

        let file_path = create_test_file(&dir, "many-comments.yaml", content.as_bytes()).await?;
        for mode in [LOAD_SINGLE_DOCUMENT, CheckMode::SyntaxOnly] {
            let result = check_file(Path::new(""), &file_path, mode).await?;
            assert_eq!(
                result.exit_status,
                0,
                "{}",
                String::from_utf8_lossy(&result.output)
            );
            assert!(result.output.is_empty());
        }
        Ok(())
    }
}
