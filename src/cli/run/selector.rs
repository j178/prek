use std::borrow::Cow;
use std::fmt::Display;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::hook::Hook;
use crate::warn_user;

use anyhow::anyhow;
use constants::env_vars::EnvVars;
use rustc_hash::FxHashSet;
use tracing::trace;

#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error("Invalid selector: `{selector}`")]
    InvalidSelector {
        selector: String,
        source: anyhow::Error,
    },

    #[error("Invalid project path: `{path}`")]
    InvalidPath {
        path: String,
        #[source]
        source: std::io::Error,
    },
}

#[derive(Debug, Clone, Copy)]
enum SelectorSource {
    CliArg,
    CliFlag(&'static str),
    EnvVar(&'static str),
}

#[derive(Debug, Clone)]
pub(crate) enum SelectorExpr {
    HookId(String),
    ProjectPrefix(PathBuf),
    ProjectHook {
        project_path: PathBuf,
        hook_id: String,
    },
}

#[derive(Debug, Clone)]
pub(crate) struct Selector {
    source: SelectorSource,
    original: String,
    expr: SelectorExpr,
}

impl Display for Selector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.expr {
            SelectorExpr::HookId(hook_id) => write!(f, ":{hook_id}"),
            SelectorExpr::ProjectPrefix(project_path) => {
                if project_path.as_os_str().is_empty() {
                    write!(f, "./")
                } else {
                    write!(f, "{}/", project_path.display())
                }
            }
            SelectorExpr::ProjectHook {
                project_path,
                hook_id,
            } => {
                if project_path.as_os_str().is_empty() {
                    write!(f, ".:{hook_id}")
                } else {
                    write!(f, "{}:{hook_id}", project_path.display())
                }
            }
        }
    }
}

impl Selector {
    pub(crate) fn as_flag(&self) -> Cow<'_, str> {
        match &self.source {
            SelectorSource::CliArg => Cow::Borrowed(&self.original),
            SelectorSource::CliFlag(flag) => Cow::Owned(format!("{}={}", flag, self.original)),
            SelectorSource::EnvVar(var) => Cow::Owned(format!("{}={}", var, self.original)),
        }
    }

    pub(crate) fn as_normalized_flag(&self) -> String {
        match &self.source {
            SelectorSource::CliArg => self.to_string(),
            SelectorSource::CliFlag(flag) => format!("{flag}={self}"),
            SelectorSource::EnvVar(var) => format!("{var}={self}"),
        }
    }
}

impl Selector {
    pub(crate) fn matches_hook(&self, hook: &Hook) -> bool {
        match &self.expr {
            SelectorExpr::HookId(hook_id) => {
                // For bare hook IDs, check if it matches the hook
                hook.id == *hook_id || hook.alias == *hook_id
            }
            SelectorExpr::ProjectPrefix(project_path) => {
                // For project paths, check if the hook belongs to that project.
                hook.project().relative_path().starts_with(project_path)
            }
            SelectorExpr::ProjectHook {
                project_path,
                hook_id,
            } => {
                // For project:hook syntax, check both
                (hook.id == *hook_id || hook.alias == *hook_id)
                    && project_path == hook.project().relative_path()
            }
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Selectors {
    includes: Vec<Selector>,
    skips: Vec<Selector>,
    usage: Arc<Mutex<SelectorUsage>>,
}

impl Selectors {
    /// Load include and skip selectors from CLI args and environment variables.
    pub(crate) fn load(
        includes: &[String],
        skips: &[String],
        workspace_root: &Path,
    ) -> Result<Selectors, Error> {
        let includes = includes
            .iter()
            .map(|selector| parse_single_selector(selector, workspace_root, SelectorSource::CliArg))
            .collect::<Result<Vec<_>, _>>()?;

        trace!(
            "Include selectors: {}",
            includes
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        );

        let skips = load_skips(skips, workspace_root)?;

        trace!(
            "Skip selectors: {}",
            skips
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        );

        Ok(Self {
            includes,
            skips,
            usage: Arc::default(),
        })
    }

    /// Check if a hook matches any of the selection criteria
    pub(crate) fn matches_hook(&self, hook: &Hook) -> bool {
        let mut usage = self.usage.lock().unwrap();

        // Always check every selector to track usage
        let mut skipped = false;
        for (idx, skip) in self.skips.iter().enumerate() {
            if skip.matches_hook(hook) {
                usage.use_skip(idx);
                skipped = true;
            }
        }
        if skipped {
            return false;
        }

        if self.includes.is_empty() {
            return true; // No `includes` mean all hooks are included
        }

        let mut included = false;
        for (idx, include) in self.includes.iter().enumerate() {
            if include.matches_hook(hook) {
                usage.use_include(idx);
                included = true;
            }
        }
        included
    }

    pub(crate) fn matches_path(&self, path: &Path) -> bool {
        let mut usage = self.usage.lock().unwrap();

        let mut skipped = false;
        for (idx, skip) in self.skips.iter().enumerate() {
            if let SelectorExpr::ProjectPrefix(project_path) = &skip.expr {
                if path.starts_with(project_path) {
                    usage.use_skip(idx);
                    skipped = true;
                }
            }
        }
        if skipped {
            return false;
        }

        // If no project prefix selectors are present, all paths are included
        if !self
            .includes
            .iter()
            .any(|include| matches!(include.expr, SelectorExpr::ProjectPrefix(_)))
        {
            return true;
        }

        let mut included = false;
        for (idx, include) in self.includes.iter().enumerate() {
            if let SelectorExpr::ProjectPrefix(project_path) = &include.expr {
                if path.starts_with(project_path) {
                    usage.use_include(idx);
                    included = true;
                }
            }
        }
        included
    }

    pub(crate) fn report_unused(&self) {
        let usage = self.usage.lock().unwrap();
        usage.report_unused(self);
    }
}

#[derive(Default, Debug)]
struct SelectorUsage {
    used_includes: FxHashSet<usize>,
    used_skips: FxHashSet<usize>,
}

impl SelectorUsage {
    fn use_include(&mut self, idx: usize) {
        self.used_includes.insert(idx);
    }

    fn use_skip(&mut self, idx: usize) {
        self.used_skips.insert(idx);
    }

    fn report_unused(&self, selectors: &Selectors) {
        let unused = selectors
            .includes
            .iter()
            .enumerate()
            .filter(|(idx, _)| !self.used_includes.contains(idx))
            .chain(
                selectors
                    .skips
                    .iter()
                    .enumerate()
                    .filter(|(idx, _)| !self.used_skips.contains(idx)),
            )
            .collect::<Vec<_>>();

        match unused.as_slice() {
            [] => {}
            [(_, selector)] => {
                let flag = selector.as_flag();
                let normalized = selector.as_normalized_flag();
                if flag == normalized {
                    warn_user!("selector `{flag}` did not match any hooks or projects",);
                } else {
                    warn_user!(
                        "selector `{flag}` ({}) did not match any hooks or projects",
                        format!("normalized to `{normalized}`").dimmed()
                    );
                }
            }
            _ => {
                let warning = unused
                    .iter()
                    .map(|(_, sel)| {
                        let flag = sel.as_flag();
                        let normalized = sel.as_normalized_flag();
                        if flag == normalized {
                            format!("  - `{flag}`")
                        } else {
                            format!(
                                "  - `{flag}` ({})",
                                format!("normalized to `{normalized}`").dimmed()
                            )
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n");

                warn_user!("the following selectors did not match any hooks or projects:");
                anstream::eprintln!("{warning}");
            }
        }
    }
}

/// Parse a single selector string into a Selection enum.
fn parse_single_selector(
    input: &str,
    workspace_root: &Path,
    source: SelectorSource,
) -> Result<Selector, Error> {
    if input.chars().filter(|&c| c == ':').count() > 1 {
        return Err(Error::InvalidSelector {
            selector: input.to_string(),
            source: anyhow!("only one ':' is allowed"),
        });
    }

    // Handle explicit hook ID with : prefix
    if let Some(hook_id) = input.strip_prefix(':') {
        if hook_id.is_empty() {
            return Err(Error::InvalidSelector {
                selector: input.to_string(),
                source: anyhow!("hook ID part is empty"),
            });
        }
        return Ok(Selector {
            source,
            original: input.to_string(),
            expr: SelectorExpr::HookId(hook_id.to_string()),
        });
    }

    // Handle `project:hook` syntax
    if let Some((project_path, hook_id)) = input.split_once(':') {
        if project_path.is_empty() {
            return Err(Error::InvalidSelector {
                selector: input.to_string(),
                source: anyhow!("project path part is empty"),
            });
        }
        if hook_id.is_empty() {
            return Err(Error::InvalidSelector {
                selector: input.to_string(),
                source: anyhow!("hook ID part is empty"),
            });
        }

        let project_path = normalize_path(project_path, workspace_root)?;

        return Ok(Selector {
            source,
            original: input.to_string(),
            expr: SelectorExpr::ProjectHook {
                project_path,
                hook_id: hook_id.to_string(),
            },
        });
    }

    // Handle project paths
    if input == "." || input.contains('/') {
        let project_path = normalize_path(input, workspace_root)?;

        return Ok(Selector {
            source,
            original: input.to_string(),
            expr: SelectorExpr::ProjectPrefix(project_path),
        });
    }

    // Ambiguous case: treat as hook ID for backward compatibility
    if input.is_empty() {
        return Err(Error::InvalidSelector {
            selector: input.to_string(),
            source: anyhow!("cannot be empty"),
        });
    }
    Ok(Selector {
        source,
        original: input.to_string(),
        expr: SelectorExpr::HookId(input.to_string()),
    })
}

/// Normalize a project path to the relative path from the workspace root.
/// In workspace root:
/// './project/' -> 'project'
/// 'project/sub/' -> 'project/sub'
/// '.' -> ''
/// './' -> ''
/// '..' -> Error
/// '../project/' -> Error
/// '/absolute/path/' -> if inside workspace, relative path; else Error
/// In subdirectory of workspace (e.g., 'workspace/subdir'):
/// './project/' -> 'subdir/project'
/// 'project/' -> 'subdir/project'
/// '../project/' -> 'project'
/// '..' -> ''
fn normalize_path(path: &str, workspace_root: &Path) -> Result<PathBuf, Error> {
    let canonicalize_path = std::fs::canonicalize(path).map_err(|e| Error::InvalidSelector {
        selector: path.to_string(),
        source: anyhow!(e),
    })?;

    let rel_path = canonicalize_path
        .strip_prefix(workspace_root)
        .map_err(|_| Error::InvalidSelector {
            selector: path.to_string(),
            source: anyhow!("path is outside the workspace root"),
        })?;

    Ok(rel_path.to_path_buf())
}

/// Parse skip selectors from CLI args and environment variables
pub(crate) fn load_skips(
    cli_skips: &[String],
    workspace_root: &Path,
) -> Result<Vec<Selector>, Error> {
    let prek_skip = EnvVars::var(EnvVars::PREK_SKIP);
    let skip = EnvVars::var(EnvVars::SKIP);

    let (skips, source) = if !cli_skips.is_empty() {
        (
            cli_skips.iter().map(String::as_str).collect::<Vec<_>>(),
            SelectorSource::CliFlag("--skip"),
        )
    } else if let Ok(s) = &prek_skip {
        (
            parse_comma_separated(s).collect(),
            SelectorSource::EnvVar(EnvVars::PREK_SKIP),
        )
    } else if let Ok(s) = &skip {
        (
            parse_comma_separated(s).collect(),
            SelectorSource::EnvVar(EnvVars::SKIP),
        )
    } else {
        return Ok(vec![]);
    };

    skips
        .into_iter()
        .map(|skip| parse_single_selector(skip, workspace_root, source))
        .collect()
}

/// Parse comma-separated values, trimming whitespace and filtering empty strings
fn parse_comma_separated(input: &str) -> impl Iterator<Item = &str> {
    input.split(',').map(str::trim).filter(|s| !s.is_empty())
}
