use std::path::Path;

use anyhow::{Context, Result};
use itertools::Itertools;
use lazy_regex::regex;
use toml_edit::DocumentMut;

use crate::fs::Simplified;
use crate::yaml::serialize_yaml_scalar;

use super::{FrozenCommentSite, FrozenRef, Revision};

fn parse_frozen_ref(line: &str, line_number: usize) -> FrozenRef {
    let Some(captures) = regex!(r#"#\s*frozen:\s*([^\s#]+)"#).captures(line) else {
        return FrozenRef {
            line_number,
            current_frozen: None,
            site: None,
        };
    };
    let frozen_match = captures.get(1).expect("capture group 1 must exist");
    FrozenRef {
        line_number,
        current_frozen: Some(frozen_match.as_str().to_string()),
        site: Some(FrozenCommentSite {
            line_number,
            source_line: line.to_string(),
            span: frozen_match.start()..frozen_match.end(),
        }),
    }
}

pub(super) fn read_frozen_refs(path: &Path) -> Result<Vec<FrozenRef>> {
    let content = fs_err::read_to_string(path)?;

    match path.extension() {
        Some(ext) if ext.eq_ignore_ascii_case("toml") => Ok(content
            .lines()
            .enumerate()
            .filter(|(_, line)| regex!(r#"^\s*rev\s*="#).is_match(line))
            .map(|(index, line)| parse_frozen_ref(line, index + 1))
            .collect()),
        _ => {
            let rev_regex = regex!(r#"^\s+rev:\s*['"]?[^\s#]+(?P<comment>.*)$"#);
            Ok(content
                .lines()
                .enumerate()
                .filter_map(|(index, line)| {
                    rev_regex
                        .captures(line)
                        .map(|_| parse_frozen_ref(line, index + 1))
                })
                .collect())
        }
    }
}

fn inline_comment_spacing(comment: &str) -> Option<&str> {
    let comment_index = comment.find('#')?;
    let (spacing, _) = comment.split_at(comment_index);
    spacing.chars().all(char::is_whitespace).then_some(spacing)
}

/// Rewrites one config file with the resolved revisions for its remote repos.
pub(super) async fn write_new_config(path: &Path, revisions: &[Option<Revision>]) -> Result<()> {
    let content = fs_err::tokio::read_to_string(path).await?;
    let new_content = match path.extension() {
        Some(ext) if ext.eq_ignore_ascii_case("toml") => {
            render_updated_toml_config(path, &content, revisions)?
        }
        _ => render_updated_yaml_config(path, &content, revisions)?,
    };

    fs_err::tokio::write(path, new_content)
        .await
        .with_context(|| {
            format!(
                "Failed to write updated config file `{}`",
                path.user_display()
            )
        })?;

    Ok(())
}

/// Updates `rev` values and `# frozen:` comments in a TOML config while preserving formatting.
pub(super) fn render_updated_toml_config(
    path: &Path,
    content: &str,
    revisions: &[Option<Revision>],
) -> Result<String> {
    let mut doc = content.parse::<DocumentMut>()?;
    let Some(repos) = doc
        .get_mut("repos")
        .and_then(|item| item.as_array_of_tables_mut())
    else {
        anyhow::bail!("Missing `[[repos]]` array in `{}`", path.user_display());
    };

    let mut remote_repos = Vec::new();
    for table in repos.iter_mut() {
        let repo_value = table
            .get("repo")
            .and_then(|item| item.as_value())
            .and_then(|value| value.as_str())
            .unwrap_or_default();

        if matches!(repo_value, "local" | "meta" | "builtin") {
            continue;
        }

        if !table.contains_key("rev") {
            anyhow::bail!(
                "Found remote repo without `rev` in `{}`",
                path.user_display()
            );
        }

        remote_repos.push(table);
    }

    if remote_repos.len() != revisions.len() {
        anyhow::bail!(
            "Found {} remote repos in `{}` but expected {}, file content may have changed",
            remote_repos.len(),
            path.user_display(),
            revisions.len()
        );
    }

    for (table, revision) in remote_repos.into_iter().zip_eq(revisions) {
        let Some(revision) = revision else {
            continue;
        };

        let Some(value) = table.get_mut("rev").and_then(|item| item.as_value_mut()) else {
            continue;
        };

        let current_suffix = value.decor().suffix().and_then(|s| s.as_str());
        let frozen_spacing = current_suffix
            .and_then(inline_comment_spacing)
            .unwrap_or("  ")
            .to_string();
        let suffix = current_suffix
            .filter(|s| !s.trim_start().starts_with("# frozen:"))
            .map(str::to_string);

        *value = toml_edit::Value::from(revision.rev.clone());

        if let Some(frozen) = &revision.frozen {
            value
                .decor_mut()
                .set_suffix(format!("{frozen_spacing}# frozen: {frozen}"));
        } else if let Some(suffix) = suffix {
            value.decor_mut().set_suffix(suffix);
        }
    }

    Ok(doc.to_string())
}

/// Updates `rev` values and `# frozen:` comments in a YAML config while preserving line layout.
pub(super) fn render_updated_yaml_config(
    path: &Path,
    content: &str,
    revisions: &[Option<Revision>],
) -> Result<String> {
    let mut lines = content
        .split_inclusive('\n')
        .map(ToString::to_string)
        .collect::<Vec<_>>();

    let rev_regex = regex!(r#"^(\s+)rev:(\s*)(['"]?)([^\s#]+)(.*)(\r?\n)$"#);

    let rev_lines = lines
        .iter()
        .enumerate()
        .filter_map(|(line_no, line)| {
            if rev_regex.is_match(line) {
                Some(line_no)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();

    if rev_lines.len() != revisions.len() {
        anyhow::bail!(
            "Found {} `rev:` lines in `{}` but expected {}, file content may have changed",
            rev_lines.len(),
            path.user_display(),
            revisions.len()
        );
    }

    for (line_no, revision) in rev_lines.iter().zip_eq(revisions) {
        let Some(revision) = revision else {
            continue;
        };

        let caps = rev_regex
            .captures(&lines[*line_no])
            .context("Failed to capture rev line")?;

        let new_rev = serialize_yaml_scalar(&revision.rev, &caps[3])?;

        let comment = if let Some(frozen) = &revision.frozen {
            format!(
                "{}# frozen: {frozen}",
                inline_comment_spacing(&caps[5]).unwrap_or("  ")
            )
        } else if caps[5].trim_start().starts_with("# frozen:") {
            String::new()
        } else {
            caps[5].to_string()
        };

        lines[*line_no] = format!(
            "{}rev:{}{}{}{}",
            &caps[1], &caps[2], new_rev, comment, &caps[6]
        );
    }

    Ok(lines.join(""))
}

#[cfg(test)]
mod tests {
    use super::{render_updated_toml_config, render_updated_yaml_config};
    use crate::cli::auto_update::Revision;
    use std::path::Path;

    #[test]
    fn test_render_updated_yaml_config_uses_default_spacing_for_new_frozen_comment() {
        let config = indoc::indoc! {r"
            repos:
              - repo: https://example.com/repo
                rev: v1.0.0
                hooks:
                  - id: test-hook
        "};

        let rendered = render_updated_yaml_config(
            Path::new(".pre-commit-config.yaml"),
            config,
            &[Some(Revision {
                rev: "abc123".to_string(),
                frozen: Some("v1.1.0".to_string()),
            })],
        )
        .unwrap();

        assert!(rendered.contains("rev: abc123  # frozen: v1.1.0\n"));
    }

    #[test]
    fn test_render_updated_yaml_config_preserves_existing_frozen_comment_spacing() {
        let config = indoc::indoc! {r"
            repos:
              - repo: https://example.com/repo
                rev: v1.0.0   # frozen: v1.0.0
                hooks:
                  - id: test-hook
        "};

        let rendered = render_updated_yaml_config(
            Path::new(".pre-commit-config.yaml"),
            config,
            &[Some(Revision {
                rev: "abc123".to_string(),
                frozen: Some("v1.1.0".to_string()),
            })],
        )
        .unwrap();

        assert!(rendered.contains("rev: abc123   # frozen: v1.1.0\n"));
    }

    #[test]
    fn test_render_updated_toml_config_preserves_existing_frozen_comment_spacing() {
        let config = indoc::indoc! {r#"
            [[repos]]
            repo = "https://example.com/repo"
            rev = "v1.0.0" # frozen: v1.0.0
            hooks = [{ id = "test-hook" }]
        "#};

        let rendered = render_updated_toml_config(
            Path::new("prek.toml"),
            config,
            &[Some(Revision {
                rev: "abc123".to_string(),
                frozen: Some("v1.1.0".to_string()),
            })],
        )
        .unwrap();

        assert!(rendered.contains(r#"rev = "abc123" # frozen: v1.1.0"#));
    }
}
