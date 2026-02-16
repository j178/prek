use std::io;
use std::path::Path;

use anyhow::Result;
use serde::Deserialize;
use tracing::trace;

use crate::config::Language;
use crate::hook::Hook;
use crate::languages::version::LanguageRequest;

#[derive(Debug, Deserialize)]
struct PyProjectToml {
    project: Option<ProjectTable>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct ProjectTable {
    requires_python: Option<String>,
}

async fn extract_pyproject_requires_python(repo_path: &Path) -> Result<Option<String>> {
    let pyproject = repo_path.join("pyproject.toml");
    let contents = match fs_err::tokio::read_to_string(&pyproject).await {
        Ok(contents) => contents,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err.into()),
    };

    let parsed = match toml::from_str::<PyProjectToml>(&contents) {
        Ok(parsed) => parsed,
        Err(err) => {
            trace!(error = %err, "Ignoring unparsable pyproject.toml");
            return Ok(None);
        }
    };

    Ok(parsed.project.and_then(|project| project.requires_python))
}

/// Extract `requires-python` from the hook repo's `pyproject.toml`.
///
/// Only acts when `language_request` is still `Any` (i.e. no explicit
/// `language_version` was configured by the user).
pub(crate) async fn extract_pyproject_metadata(hook: &mut Hook) -> Result<()> {
    if !hook.language_request.is_any() {
        trace!(
            hook = %hook,
            "Skipping pyproject.toml metadata extraction because language_version is already configured",
        );
        return Ok(());
    }

    let Some(repo_path) = hook.repo_path() else {
        return Ok(());
    };

    let Some(req_str) = extract_pyproject_requires_python(repo_path).await? else {
        trace!(hook = %hook, "No requires-python found in pyproject.toml");
        return Ok(());
    };

    let req = match LanguageRequest::parse(Language::Python, &req_str) {
        Ok(req) => req,
        Err(err) => {
            trace!(%req_str, error = %err, "Ignoring invalid pyproject.toml requires-python");
            return Ok(());
        }
    };

    trace!(hook = %hook, version = %req_str, "Using pyproject.toml-derived language_version");
    hook.language_request = req;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn valid_requires_python() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        fs_err::tokio::write(
            dir.path().join("pyproject.toml"),
            "[project]\nrequires-python = \">=3.10\"\n",
        )
        .await?;

        let req = extract_pyproject_requires_python(dir.path()).await?;
        assert_eq!(req.as_deref(), Some(">=3.10"));
        Ok(())
    }

    #[tokio::test]
    async fn missing_file_returns_none() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let req = extract_pyproject_requires_python(dir.path()).await?;
        assert!(req.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn missing_project_table_returns_none() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        fs_err::tokio::write(
            dir.path().join("pyproject.toml"),
            "[build-system]\nrequires = [\"setuptools\"]\n",
        )
        .await?;

        let req = extract_pyproject_requires_python(dir.path()).await?;
        assert!(req.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn missing_requires_python_returns_none() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        fs_err::tokio::write(
            dir.path().join("pyproject.toml"),
            "[project]\nname = \"my-project\"\n",
        )
        .await?;

        let req = extract_pyproject_requires_python(dir.path()).await?;
        assert!(req.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn unparsable_toml_returns_none() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        fs_err::tokio::write(
            dir.path().join("pyproject.toml"),
            "this is not valid toml {{{\n",
        )
        .await?;

        let req = extract_pyproject_requires_python(dir.path()).await?;
        assert!(req.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn invalid_version_specifier_is_ignored() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        fs_err::tokio::write(
            dir.path().join("pyproject.toml"),
            "[project]\nrequires-python = \"not a valid specifier\"\n",
        )
        .await?;

        let req = extract_pyproject_requires_python(dir.path()).await?;
        assert_eq!(req.as_deref(), Some("not a valid specifier"));

        // The string is returned, but LanguageRequest::parse would reject it.
        // extract_pyproject_metadata handles that gracefully (trace + return Ok(())).
        let parse_result = LanguageRequest::parse(Language::Python, "not a valid specifier");
        assert!(parse_result.is_err());

        Ok(())
    }
}
