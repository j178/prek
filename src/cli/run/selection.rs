use std::fmt::Display;
use std::path::Path;

use crate::hook::Hook;
use crate::workspace::Workspace;

#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error("Invalid selector: `{selector}`: {reason}")]
    InvalidSelector {
        selector: String,
        reason: &'static str,
    },
}

#[derive(Debug, Clone)]
pub(crate) enum Selection {
    HookId(String),
    ProjectPath(String),
    ProjectHook {
        project_path: String,
        hook_id: String,
    },
}

impl Display for Selection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Selection::HookId(hook_id) => write!(f, ":{hook_id}"),
            Selection::ProjectPath(project_path) => write!(f, "{project_path}"),
            Selection::ProjectHook {
                project_path,
                hook_id,
            } => write!(f, "{project_path}:{hook_id}"),
        }
    }
}

impl Selection {
    pub(crate) fn matches_hook(&self, hook: &Hook, workspace: &Workspace) -> bool {
        match self {
            Selection::HookId(hook_id) => {
                // For bare hook IDs, check if it matches the hook
                hook.id == *hook_id || hook.alias == *hook_id
            }
            Selection::ProjectPath(project_path) => {
                // For project paths, check if the hook belongs to that project
                Self::project_matches(hook, project_path, workspace)
            }
            Selection::ProjectHook {
                project_path,
                hook_id,
            } => {
                // For project:hook syntax, check both
                (hook.id == *hook_id || hook.alias == *hook_id)
                    && Self::project_matches(hook, project_path, workspace)
            }
        }
    }

    /// Check if a hook belongs to a specific project
    fn project_matches(hook: &Hook, project_path: &str, workspace: &Workspace) -> bool {
        workspace.projects().iter().any(|project| {
            project.relative_path() == Path::new(project_path) && hook.project() == &**project
        })
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Selections {
    selections: Vec<Selection>,
    skips: Vec<Selection>,
}

impl Selections {
    /// Parse hook/project selections.
    pub(crate) fn from_args(selectors: &[String], skips: &[String]) -> Result<Selections, Error> {
        let selections = selectors
            .iter()
            .map(|selector| parse_single_selection(selector))
            .collect::<Result<_, _>>()?;
        let skips = skips
            .iter()
            .map(|skip| parse_single_selection(skip))
            .collect::<Result<_, _>>()?;

        Ok(Self { selections, skips })
    }

    /// Check if a hook matches any of the selection criteria
    pub(crate) fn matches_hook(&self, hook: &Hook, workspace: &Workspace) -> bool {
        if self
            .skips
            .iter()
            .any(|skip| skip.matches_hook(hook, workspace))
        {
            return false;
        }

        if self.selections.is_empty() {
            return true; // No selections mean all hooks are included
        }

        if self
            .selections
            .iter()
            .any(|selection| selection.matches_hook(hook, workspace))
        {
            return true;
        }

        false
    }

    pub(crate) fn is_path_selected(&self, path: &Path) -> bool {
        if self
            .skips
            .iter()
            .any(|skip| matches!(skip, Selection::ProjectPath(p) if path == Path::new(p)))
        {
            return false;
        }
        if self.selections.is_empty() {
            return true; // No selections mean all paths are included
        }
        if self
            .selections
            .iter()
            .any(|selection| matches!(selection, Selection::ProjectPath(p) if path == Path::new(p)))
        {
            return true;
        }
        false
    }
}

/// Parse a single selection string into a Selection enum
fn parse_single_selection(input: &str) -> Result<Selection, Error> {
    if input.chars().filter(|&c| c == ':').count() > 1 {
        return Err(Error::InvalidSelector {
            selector: input.to_string(),
            reason: "only one ':' is allowed",
        });
    }

    // Handle explicit hook ID with : prefix
    if let Some(hook_id) = input.strip_prefix(':') {
        if hook_id.is_empty() {
            return Err(Error::InvalidSelector {
                selector: input.to_string(),
                reason: "hook ID part is empty",
            });
        }
        return Ok(Selection::HookId(hook_id.to_string()));
    }

    // Handle `project:hook` syntax
    if let Some((project, hook)) = input.split_once(':') {
        if project.is_empty() {
            return Err(Error::InvalidSelector {
                selector: input.to_string(),
                reason: "project path part is empty",
            });
        }
        if hook.is_empty() {
            return Err(Error::InvalidSelector {
                selector: input.to_string(),
                reason: "hook ID part is empty",
            });
        }
        let project = project.trim_start_matches("./").trim_end_matches('/');
        return Ok(Selection::ProjectHook {
            project_path: project.to_string(),
            hook_id: hook.to_string(),
        });
    }

    // Handle project paths
    if input == "." || input.contains('/') {
        let project_path = input
            .trim_start_matches("./")
            .trim_end_matches('/')
            .to_string();
        return Ok(Selection::ProjectPath(project_path));
    }

    // Ambiguous case: treat as hook ID for backward compatibility
    if input.is_empty() {
        return Err(Error::InvalidSelector {
            selector: input.to_string(),
            reason: "cannot be empty",
        });
    }

    Ok(Selection::HookId(input.to_string()))
}

/// Get skip values from CLI args and environment variables
pub(crate) fn get_skips(cli_skip: &[String]) -> Vec<String> {
    use constants::env_vars::EnvVars;

    // command line arguments overwrite env vars
    if !cli_skip.is_empty() {
        return cli_skip.to_vec();
    }

    if let Some(s) = EnvVars::var_os(EnvVars::SKIP) {
        return parse_comma_separated(&s.to_string_lossy()).collect();
    }

    if let Some(s) = EnvVars::var_os(EnvVars::PREK_SKIP) {
        return parse_comma_separated(&s.to_string_lossy()).collect();
    }

    vec![]
}

/// Parse comma-separated values, trimming whitespace and filtering empty strings
fn parse_comma_separated(input: &str) -> impl Iterator<Item = String> + '_ {
    input
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
}
