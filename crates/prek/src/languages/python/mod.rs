use anyhow::Result;

use crate::languages::HookMetadata;

mod pep723;
mod pyproject;
#[allow(clippy::module_inception)]
mod python;
mod uv;
mod version;

/// Extract Python hook metadata with explicit precedence:
/// PEP 723 > user-configured `language_version` > pyproject.toml > default.
pub(crate) async fn extract_metadata(metadata: &mut HookMetadata<'_>) -> Result<()> {
    pyproject::extract_pyproject_metadata(metadata).await?;
    pep723::extract_pep723_metadata(metadata).await
}

pub(crate) use python::{Python, PythonUv};
pub(crate) use python::{python_exec, query_python_info_cached};
pub(crate) use uv::Uv;
pub(crate) use version::PythonRequest;
