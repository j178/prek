use std::cmp::Reverse;
use std::ops::Range;
use std::path::Path;
use std::sync::LazyLock;

use anyhow::{Context, Result};
use granit_parser::{Event, Parser, ScalarStyle, StructureStyle};
use regex::Regex;
use rustc_hash::FxHashMap;
use serde::Deserialize;
use serde_saphyr::Spanned;
use toml_edit::{Document, TableLike};

use crate::fs::Simplified;
use crate::yaml::serialize_yaml_scalar;

use super::{FrozenCommentSite, FrozenRef, Revision};

static FROZEN_REF_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"#\s*frozen:\s*([^\s#]+)").expect("frozen ref regex must be valid")
});

#[derive(Clone, Copy)]
enum RevisionFormat {
    Yaml { quote: &'static str, flow: bool },
    Toml,
    Unsupported(&'static str),
}

struct RevisionSite {
    value: Range<usize>,
    comment: Option<Range<usize>>,
    format: RevisionFormat,
}

#[derive(Deserialize)]
struct YamlConfig {
    #[serde(default)]
    repos: Vec<YamlRepo>,
}

#[derive(Deserialize)]
struct YamlRepo {
    repo: String,
    rev: Option<Spanned<String>>,
}

fn yaml_revision_sites(content: &str) -> Result<Vec<RevisionSite>> {
    let config: YamlConfig = serde_saphyr::from_str(content)?;
    let mut formats = FxHashMap::default();
    let mut comments = Vec::new();
    let mut containers = Vec::new();

    // Serde resolves aliases and merges. Parser events retain the source style and
    // anchors needed to decide whether a scalar can be edited independently.
    for event in Parser::new_from_str(content) {
        let (event, span) = event?;
        match event {
            Event::MappingStart(style, anchor, _) | Event::SequenceStart(style, anchor, _) => {
                containers.push((style, anchor));
            }
            Event::MappingEnd | Event::SequenceEnd => {
                containers.pop();
            }
            Event::Scalar(_, style, anchor, _) => {
                let span = span
                    .byte_range()
                    .context("Missing YAML scalar source range")?;
                let format = if anchor != 0 || containers.iter().any(|(_, anchor)| *anchor != 0) {
                    RevisionFormat::Unsupported("anchored YAML values")
                } else {
                    let flow = containers
                        .iter()
                        .any(|(style, _)| *style == StructureStyle::Flow);
                    match style {
                        ScalarStyle::Plain => RevisionFormat::Yaml { quote: "", flow },
                        ScalarStyle::SingleQuoted => RevisionFormat::Yaml { quote: "'", flow },
                        ScalarStyle::DoubleQuoted => RevisionFormat::Yaml { quote: "\"", flow },
                        ScalarStyle::Literal | ScalarStyle::Folded => {
                            RevisionFormat::Unsupported("YAML block scalars")
                        }
                    }
                };
                formats.insert(span.start, format);
            }
            Event::Comment(_, _) => {
                comments.push(
                    span.byte_range()
                        .context("Missing YAML comment source range")?,
                );
            }
            _ => {}
        }
    }

    let mut sites = Vec::new();
    for repo in config.repos {
        if matches!(repo.repo.as_str(), "local" | "meta" | "builtin") {
            continue;
        }
        let rev = repo.rev.context("Missing remote repository revision")?;
        let span = rev.referenced.span();
        let start = usize::try_from(
            span.byte_offset()
                .context("Missing YAML revision source offset")?,
        )?;
        let len = usize::try_from(
            span.byte_len()
                .context("Missing YAML revision source length")?,
        )?;
        let value = start..start + len;
        let format = if rev.referenced != rev.defined {
            RevisionFormat::Unsupported("YAML aliases and merge keys")
        } else {
            *formats
                .get(&start)
                .context("Missing YAML revision scalar")?
        };
        let comment = comments
            .iter()
            .find(|comment| {
                comment.start >= value.end
                    && matches!(
                        content[value.end..comment.start].trim_matches([' ', '\t']),
                        "" | ","
                    )
            })
            .cloned();
        sites.push(RevisionSite {
            value,
            comment,
            format,
        });
    }
    Ok(sites)
}

fn toml_revision_sites(content: &str) -> Result<Vec<RevisionSite>> {
    let doc = Document::parse(content)?;
    let Some(repos) = doc.get("repos") else {
        return Ok(Vec::new());
    };
    let tables: Vec<&dyn TableLike> = if let Some(tables) = repos.as_array_of_tables() {
        tables.iter().map(|table| table as &dyn TableLike).collect()
    } else if let Some(array) = repos.as_array() {
        array
            .iter()
            .map(|value| {
                value
                    .as_inline_table()
                    .map(|table| table as &dyn TableLike)
                    .context("Expected a repository table")
            })
            .collect::<Result<_>>()?
    } else {
        anyhow::bail!("Expected a `repos` array");
    };

    let mut sites = Vec::new();
    for table in tables {
        if matches!(
            table.get("repo").and_then(toml_edit::Item::as_str),
            Some("local" | "meta" | "builtin")
        ) {
            continue;
        }
        let value = table
            .get("rev")
            .and_then(toml_edit::Item::as_value)
            .context("Missing remote repository revision")?;
        let span = value.span().context("Missing TOML revision source range")?;
        let comment = value
            .decor()
            .suffix()
            .and_then(toml_edit::RawString::span)
            .and_then(|suffix| {
                let text = &content[suffix.clone()];
                let start = text.find('#')?;
                let end = text[start..]
                    .find(['\r', '\n'])
                    .map_or(text.len(), |end| start + end);
                Some(suffix.start + start..suffix.start + end)
            });
        sites.push(RevisionSite {
            value: span,
            comment,
            format: RevisionFormat::Toml,
        });
    }
    Ok(sites)
}

fn revision_sites(path: &Path, content: &str) -> Result<Vec<RevisionSite>> {
    match path.extension() {
        Some(ext) if ext.eq_ignore_ascii_case("toml") => toml_revision_sites(content),
        _ => yaml_revision_sites(content),
    }
    .with_context(|| {
        format!(
            "Failed to locate repository revisions in `{}`",
            path.user_display()
        )
    })
}

fn source_line(content: &str, offset: usize) -> (usize, Range<usize>) {
    let prefix = &content[..offset];
    let line_number = prefix.bytes().filter(|byte| *byte == b'\n').count() + 1;
    let start = prefix.rfind('\n').map_or(0, |index| index + 1);
    let end = content[offset..]
        .find('\n')
        .map_or(content.len(), |index| offset + index);
    (line_number, start..end)
}

impl RevisionSite {
    fn frozen_ref(&self, content: &str) -> FrozenRef {
        let line_number = source_line(content, self.value.start).0;
        let site = self.comment.as_ref().and_then(|comment| {
            let captures = FROZEN_REF_RE.captures(&content[comment.clone()])?;
            let frozen = captures.get(1)?;
            let start = comment.start + frozen.start();
            let (line_number, line) = source_line(content, start);
            Some(FrozenCommentSite {
                line_number,
                source_line: content[line.clone()].trim_end_matches('\r').to_string(),
                span: start - line.start..comment.start + frozen.end() - line.start,
            })
        });
        FrozenRef {
            line_number,
            current_frozen: site
                .as_ref()
                .map(|site| site.source_line[site.span.clone()].to_string()),
            site,
        }
    }
}

pub(super) fn read_frozen_refs(path: &Path) -> Result<Vec<FrozenRef>> {
    let content = fs_err::read_to_string(path)?;
    Ok(revision_sites(path, &content)?
        .iter()
        .map(|site| site.frozen_ref(&content))
        .collect())
}

/// Rewrites one config file with the resolved revisions for its remote repos.
pub(super) async fn write_new_config(path: &Path, revisions: &[Option<Revision>]) -> Result<()> {
    let content = fs_err::tokio::read_to_string(path).await?;
    let new_content = render_updated_config(path, &content, revisions)?;
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

fn render_updated_config(
    path: &Path,
    content: &str,
    revisions: &[Option<Revision>],
) -> Result<String> {
    let sites = revision_sites(path, content)?;
    anyhow::ensure!(
        sites.len() == revisions.len(),
        "Found {} remote repos in `{}` but expected {}, file content may have changed",
        sites.len(),
        path.user_display(),
        revisions.len()
    );
    let mut edits = Vec::new();
    for (site, revision) in sites.iter().zip(revisions) {
        let Some(revision) = revision else {
            continue;
        };
        let rendered = match site.format {
            RevisionFormat::Yaml {
                quote: "",
                flow: true,
            } => {
                // Plain scalars have stricter quoting rules inside flow collections.
                let rendered = serde_saphyr::to_string(&serde_saphyr::FlowSeq([&revision.rev]))?;
                rendered
                    .strip_prefix('[')
                    .and_then(|value| value.strip_suffix("]\n"))
                    .context("Expected a YAML flow sequence")?
                    .to_string()
            }
            RevisionFormat::Yaml { quote, .. } => serialize_yaml_scalar(&revision.rev, quote)?,
            RevisionFormat::Toml => toml_edit::Value::from(revision.rev.clone()).to_string(),
            RevisionFormat::Unsupported(reason) => {
                anyhow::bail!(
                    "Cannot update `rev` at line {} in `{}`: {reason} cannot be edited independently; use a plain or quoted revision",
                    source_line(content, site.value.start).0,
                    path.user_display()
                );
            }
        };
        edits.push((site.value.clone(), rendered));

        if let Some(frozen) = &revision.frozen {
            if let Some(comment) = &site.comment {
                edits.push((comment.clone(), format!("# frozen: {frozen}")));
            } else {
                let end = source_line(content, site.value.end).1.end;
                let suffix = content[site.value.end..end].trim_end_matches('\r');
                anyhow::ensure!(
                    matches!(suffix.trim_matches([' ', '\t']), "" | ","),
                    "Cannot add a frozen comment to inline `rev` at line {} in `{}`; put the revision on its own line",
                    source_line(content, site.value.start).0,
                    path.user_display()
                );
                let at = site.value.end + suffix.trim_end_matches([' ', '\t']).len();
                edits.push((at..at, format!("  # frozen: {frozen}")));
            }
        } else if let Some(comment) = &site.comment {
            if content[comment.clone()]
                .strip_prefix('#')
                .is_some_and(|text| text.trim_start().starts_with("frozen:"))
            {
                let prefix = &content[site.value.end..comment.start];
                let start = site.value.end + prefix.trim_end_matches([' ', '\t']).len();
                edits.push((start..comment.end, String::new()));
            }
        }
    }

    edits.sort_unstable_by_key(|(span, _)| Reverse(span.start));
    let mut rendered = content.to_string();
    for (span, replacement) in edits {
        rendered.replace_range(span, &replacement);
    }
    Ok(rendered)
}

#[cfg(test)]
mod tests {
    use anyhow::Context;

    use super::{render_updated_config, revision_sites};
    use crate::cli::update::Revision;
    use std::path::Path;

    #[test]
    fn yaml_revisions_follow_repository_structure() -> anyhow::Result<()> {
        let config = "# 中文\r\nrepos:\r\n- repo: example\r\n  description: |\r\n    rev: leave-this-alone\r\n  'rev': 'v1.0.0' # keep\r\n- rev: v1.0.0\r\n  repo: another\r\n";
        let rendered = render_updated_config(
            Path::new("config.yaml"),
            config,
            &[
                Some(Revision {
                    rev: "v2.0.0".into(),
                    frozen: None,
                }),
                None,
            ],
        )?;
        assert_eq!(
            rendered,
            "# 中文\r\nrepos:\r\n- repo: example\r\n  description: |\r\n    rev: leave-this-alone\r\n  'rev': 'v2.0.0' # keep\r\n- rev: v1.0.0\r\n  repo: another\r\n"
        );
        Ok(())
    }

    #[test]
    fn yaml_flow_revision_escapes_delimiters() -> anyhow::Result<()> {
        let rendered = render_updated_config(
            Path::new("config.yaml"),
            "repos: [{repo: example, rev: v1.0.0, hooks: []}]\n",
            &[Some(Revision {
                rev: "v2,0".into(),
                frozen: None,
            })],
        )?;
        assert_eq!(
            rendered,
            "repos: [{repo: example, rev: \"v2,0\", hooks: []}]\n"
        );
        Ok(())
    }

    #[test]
    fn toml_revisions_follow_repository_structure() -> anyhow::Result<()> {
        let rendered = render_updated_config(
            Path::new("config.toml"),
            "# keep\nrepos = [{repo = 'example', 'rev' = 'v1.0.0'}, {repo = 'local'}]\nrev = 'unrelated'",
            &[Some(Revision {
                rev: "v2.0.0".into(),
                frozen: None,
            })],
        )?;
        assert_eq!(
            rendered,
            "# keep\nrepos = [{repo = 'example', 'rev' = \"v2.0.0\"}, {repo = 'local'}]\nrev = 'unrelated'"
        );
        Ok(())
    }

    #[test]
    fn frozen_refs_use_only_the_revision_trailing_comment() -> anyhow::Result<()> {
        for (path, config) in [
            (
                "config.yaml",
                "repos:\n- repo: example\n  # frozen: leading\n  rev: 'tag# frozen: fake' # human note\n- repo: another\n  rev: v1 # frozen: actual",
            ),
            (
                "config.toml",
                "[[repos]]\nrepo = 'example'\n# frozen: leading\n'rev' = 'tag# frozen: fake' # human note\n[[repos]]\nrepo = 'another'\nrev = 'v1' # frozen: actual",
            ),
        ] {
            let sites = revision_sites(Path::new(path), config)?;
            let refs: Vec<_> = sites.iter().map(|site| site.frozen_ref(config)).collect();
            assert_eq!(
                refs.iter()
                    .map(|r| r.current_frozen.as_deref())
                    .collect::<Vec<_>>(),
                [None, Some("actual")]
            );
            let site = refs[1]
                .site
                .as_ref()
                .context("missing frozen comment location")?;
            assert_eq!(&site.source_line[site.span.clone()], "actual");
        }
        Ok(())
    }

    #[test]
    fn yaml_shared_and_block_revisions_are_not_rewritten() -> anyhow::Result<()> {
        for config in [
            "repos:\n- repo: example\n  rev: &v v1\ncopy: *v\n",
            "shared: &v v1\nrepos:\n- repo: example\n  rev: *v\n",
            "repos:\n- &repo {repo: example, rev: v1}\ncopy: *repo\n",
            "shared: &repo {repo: example, rev: v1}\nrepos: [*repo]\n",
            "shared: &repo {repo: example, rev: v1}\nrepos:\n- <<: *repo\n",
            "repos:\n- repo: example\n  rev: |-\n    v1\n",
        ] {
            let path = Path::new("config.yaml");
            assert_eq!(render_updated_config(path, config, &[None])?, config);
            let error = render_updated_config(
                path,
                config,
                &[Some(Revision {
                    rev: "v2".into(),
                    frozen: None,
                })],
            )
            .unwrap_err();
            assert!(
                error.to_string().starts_with("Cannot update `rev`"),
                "{error:#}"
            );
        }
        Ok(())
    }

    #[test]
    fn frozen_comments_do_not_swallow_flow_delimiters() -> anyhow::Result<()> {
        let path = Path::new("config.yaml");
        let revision = Revision {
            rev: "abc123".into(),
            frozen: Some("v2".into()),
        };
        let rendered = render_updated_config(
            path,
            "repos: [{repo: example, rev: v1,\n  hooks: []}]\n",
            &[Some(revision.clone())],
        )?;
        assert_eq!(
            rendered,
            "repos: [{repo: example, rev: abc123,  # frozen: v2\n  hooks: []}]\n"
        );
        let unfrozen = render_updated_config(
            path,
            &rendered,
            &[Some(Revision {
                rev: "v2".into(),
                frozen: None,
            })],
        )?;
        assert_eq!(
            unfrozen,
            "repos: [{repo: example, rev: v2,\n  hooks: []}]\n"
        );
        let error = render_updated_config(
            path,
            "repos: [{repo: example, rev: v1}]\n",
            &[Some(revision)],
        )
        .unwrap_err();
        assert!(
            error.to_string().starts_with("Cannot add a frozen comment"),
            "{error:#}"
        );
        Ok(())
    }

    #[test]
    fn test_render_updated_yaml_config_uses_default_spacing_for_new_frozen_comment() {
        let config = indoc::indoc! {r"
            repos:
              - repo: https://example.com/repo
                rev: v1.0.0
                hooks:
                  - id: test-hook
        "};

        let rendered = render_updated_config(
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

        let rendered = render_updated_config(
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

        let rendered = render_updated_config(
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
