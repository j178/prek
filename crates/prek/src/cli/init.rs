use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use owo_colors::OwoColorize;
use prek_consts::{PRE_COMMIT_CONFIG_YAML, PREK_TOML};

use crate::cli::sample_config::write_sample_config;
use crate::cli::{ExitStatus, SampleConfigFormat, install};
use crate::fs::Simplified;
use crate::git::GIT_ROOT;
use crate::printer::Printer;
use crate::store::Store;
use crate::workspace::{Error as WorkspaceError, Project, Workspace};

pub(crate) async fn init(
    store: &Store,
    path: Option<PathBuf>,
    format: SampleConfigFormat,
    no_install: bool,
    printer: Printer,
) -> Result<ExitStatus> {
    let git_root = GIT_ROOT.as_ref()?;
    let target = resolve_target(path, git_root)?;
    let (project, created) = load_or_create_project(&target, format)?;

    if created {
        writeln!(
            printer.stdout(),
            "Created `{}`",
            project.config_file().user_display().cyan()
        )?;
    } else {
        writeln!(
            printer.stdout(),
            "Found existing `{}`; skipping creation",
            project.config_file().user_display().cyan()
        )?;
    }

    let project_roots = configured_project_roots(&target, git_root);
    for root in project_roots.iter().skip(1) {
        Workspace::invalidate_cache(store, root)?;
    }

    if no_install {
        return Ok(ExitStatus::Success);
    }

    let Some(install_root) = project_roots.last() else {
        anyhow::bail!("Failed to find the initialized configuration");
    };

    // Git has one effective hooks directory for the repository. Installing from the outermost
    // configured ancestor keeps the shim responsible for every nested project.
    install(
        store,
        install_root,
        None,
        vec![],
        vec![],
        vec![],
        false,
        false,
        false,
        false,
        printer,
        None,
    )
    .await
}

fn resolve_target(path: Option<PathBuf>, git_root: &Path) -> Result<PathBuf> {
    let Some(path) = path else {
        return Ok(git_root.to_path_buf());
    };

    let target = dunce::canonicalize(&path)
        .with_context(|| format!("Failed to resolve directory `{}`", path.user_display()))?;
    if !target.is_dir() {
        anyhow::bail!("Path `{}` is not a directory", path.user_display().cyan());
    }
    if !target.starts_with(git_root) {
        anyhow::bail!(
            "Directory `{}` is outside Git worktree `{}`",
            target.user_display().cyan(),
            git_root.user_display().cyan()
        );
    }

    Ok(target)
}

fn load_or_create_project(target: &Path, format: SampleConfigFormat) -> Result<(Project, bool)> {
    match Project::from_directory(target) {
        Ok(project) => Ok((project, false)),
        Err(WorkspaceError::MissingConfigFile) => {
            let filename = match format {
                SampleConfigFormat::Yaml => PRE_COMMIT_CONFIG_YAML,
                SampleConfigFormat::Toml => PREK_TOML,
            };
            write_sample_config(&target.join(filename), format)?;
            Ok((Project::from_directory(target)?, true))
        }
        Err(err) => Err(err.into()),
    }
}

fn configured_project_roots(target: &Path, git_root: &Path) -> Vec<PathBuf> {
    target
        .ancestors()
        .take_while(|path| path.starts_with(git_root))
        .filter(|path| Project::find_config(path).is_some())
        .map(Path::to_path_buf)
        .collect()
}
