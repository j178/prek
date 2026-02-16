use std::io;
use std::path::Path;

use anyhow::Result;
use tracing::trace;

use crate::config::Language;
use crate::hook::Hook;
use crate::languages::version::LanguageRequest;

fn parse_go_mod_directives(contents: &str) -> (Option<String>, Option<String>) {
    let mut go_version: Option<String> = None;
    let mut toolchain: Option<String> = None;

    for line in contents.lines() {
        let mut line = line.trim();
        if line.is_empty() {
            continue;
        }

        // Strip `//` comments.
        if let Some((before, _)) = line.split_once("//") {
            line = before.trim();
            if line.is_empty() {
                continue;
            }
        }

        let mut tokens = line.split_whitespace();
        let Some(directive) = tokens.next() else {
            continue;
        };
        let value = tokens.next();

        // `go 1.22.0`
        if go_version.is_none() && directive == "go" {
            if let Some(version) = value {
                go_version = Some(version.to_string());
            }
            continue;
        }

        // `toolchain go1.22.1`
        if toolchain.is_none() && directive == "toolchain" {
            if let Some(version) = value {
                // `toolchain` in go.mod does not accept `default`.
                if version != "default" {
                    toolchain = Some(version.to_string());
                }
            }
        }
    }

    (go_version, toolchain)
}

fn normalize_go_semver_min(version: &str) -> String {
    // `go.mod` commonly uses `1.23` (no patch). The semver range parser is happier when
    // we provide a full `MAJOR.MINOR.PATCH` minimum.
    let mut parts = version.split('.').collect::<Vec<_>>();
    if parts.is_empty() {
        return version.to_string();
    }

    // If any part isn't a pure integer (e.g., `1.23rc1`), keep it as-is.
    // TODO: support pre-release versions properly.
    if parts.iter().any(|p| p.parse::<u64>().is_err()) {
        return version.to_string();
    }

    match parts.len() {
        1 => {
            parts.push("0");
            parts.push("0");
        }
        2 => {
            parts.push("0");
        }
        _ => {}
    }

    parts.join(".")
}

fn choose_language_version_from_go_mod(contents: &str) -> Option<String> {
    let (go_version, toolchain) = parse_go_mod_directives(contents);

    // Prefer `go` to maximize cache reuse: it's typically stable across patch updates.
    let go_version = go_version.or(toolchain)?;
    let stripped = go_version.strip_prefix("go").unwrap_or(&go_version);
    let normalized = normalize_go_semver_min(stripped);
    Some(format!(">= {normalized}"))
}

async fn extract_go_mod_language_request(repo_path: &Path) -> Result<Option<String>> {
    let go_mod = repo_path.join("go.mod");
    let contents = match fs_err::tokio::read(&go_mod).await {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err.into()),
    };
    let contents = str::from_utf8(&contents)?;

    Ok(choose_language_version_from_go_mod(contents))
}

pub(crate) async fn extract_go_mod_metadata(hook: &mut Hook) -> Result<()> {
    // Respect an explicitly configured `language_version`.
    if !hook.language_request.is_any() {
        trace!(hook = %hook, "Skipping go.mod metadata extraction because language_version is already configured");
        return Ok(());
    }

    let Some(repo_path) = hook.repo_path() else {
        return Ok(());
    };

    let Some(req_str) = extract_go_mod_language_request(repo_path).await? else {
        trace!(hook = %hook, "No go or toolchain directive found in go.mod");
        return Ok(());
    };

    let req = match LanguageRequest::parse(Language::Golang, &req_str) {
        Ok(req) => req,
        Err(err) => {
            trace!(%req_str, error = %err, "Ignoring invalid go.mod-derived language_version");
            return Ok(());
        }
    };

    trace!(hook = %hook, version = %req_str, "Using go.mod-derived language_version");
    hook.language_request = req;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn go_line_is_used_when_only_go_present() {
        let contents = r"module example.com/foo

go 1.22.0
";
        assert_eq!(
            choose_language_version_from_go_mod(contents).as_deref(),
            Some(">= 1.22.0")
        );
    }

    #[test]
    fn go_is_preferred_over_toolchain() {
        let contents = r"module example.com/foo

go 1.22.0
toolchain go1.22.3
";
        assert_eq!(
            choose_language_version_from_go_mod(contents).as_deref(),
            Some(">= 1.22.0")
        );
    }

    #[test]
    fn invalid_toolchain_value_is_ignored() {
        let contents = r"module example.com/foo

toolchain default
";
        assert_eq!(
            choose_language_version_from_go_mod(contents).as_deref(),
            None
        );
    }

    #[test]
    fn comments_and_whitespace_are_ignored() {
        let contents = "// header

// go 1.22
go 1.20.4 // ignored
// trailing
";
        assert_eq!(
            choose_language_version_from_go_mod(contents).as_deref(),
            Some(">= 1.20.4")
        );
    }

    #[test]
    fn toolchain_is_used_when_no_go_present() {
        let contents = r"module example.com/foo

toolchain go1.23.10
";
        assert_eq!(
            choose_language_version_from_go_mod(contents).as_deref(),
            Some(">= 1.23.10")
        );
    }

    #[test]
    fn go_minor_is_normalized_to_patch() {
        let contents = r"module example.com/foo

go 1.23
";
        assert_eq!(
            choose_language_version_from_go_mod(contents).as_deref(),
            Some(">= 1.23.0")
        );
    }

    #[tokio::test]
    async fn extract_language_request_from_repo_go_line() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        fs_err::tokio::write(
            dir.path().join("go.mod"),
            "module example.com/foo\n\ngo 1.22\n",
        )
        .await?;

        let Some(req) = extract_go_mod_language_request(dir.path()).await? else {
            anyhow::bail!("Expected a language request");
        };
        assert_eq!(req, ">= 1.22.0");

        Ok(())
    }

    #[tokio::test]
    async fn extract_language_request_from_repo_toolchain_when_no_go() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        fs_err::tokio::write(
            dir.path().join("go.mod"),
            "module example.com/foo\n\ntoolchain go1.23.10\n",
        )
        .await?;

        let Some(req) = extract_go_mod_language_request(dir.path()).await? else {
            anyhow::bail!("Expected a language request");
        };

        assert_eq!(req, ">= 1.23.10");
        Ok(())
    }

    #[tokio::test]
    async fn extract_language_request_ignores_invalid_toolchain_value() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        fs_err::tokio::write(
            dir.path().join("go.mod"),
            "module example.com/foo\n\ntoolchain default\n",
        )
        .await?;

        let req = extract_go_mod_language_request(dir.path()).await?;
        assert!(req.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn extract_language_request_missing_go_mod_is_none() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let req = extract_go_mod_language_request(dir.path()).await?;
        assert!(req.is_none());
        Ok(())
    }
}
