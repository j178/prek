use std::fmt::Display;
use std::path::Path;

use crate::hook::Hook;
use crate::workspace::Workspace;

#[derive(Debug, Clone)]
pub(crate) enum Selection {
    HookId(String),
    ProjectPath(String),
    ProjectHook { project: String, hook: String },
}

impl Display for Selection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Selection::HookId(hook_id) => write!(f, ":{hook_id}"),
            Selection::ProjectPath(project_path) => write!(f, "{project_path}"),
            Selection::ProjectHook { project, hook } => write!(f, "{project}:{hook}"),
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
                project_matches(hook, project_path, workspace)
            }
            Selection::ProjectHook {
                project,
                hook: hook_id,
            } => {
                // For project:hook syntax, check both
                (hook.id == *hook_id || hook.alias == *hook_id)
                    && project_matches(hook, project, workspace)
            }
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Selections(Vec<Selection>);

impl Selections {
    /// Parse hook/project selections.
    pub(crate) fn parse(selectors: &[String]) -> Selections {
        Self(
            selectors
                .iter()
                .map(|skip| parse_single_selection(skip))
                .collect(),
        )
    }

    /// Check if a hook matches any of the selection criteria
    pub(crate) fn matches_hook(&self, hook: &Hook, workspace: &Workspace) -> bool {
        self.0
            .iter()
            .any(|selection| selection.matches_hook(hook, workspace))
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &Selection> {
        self.0.iter()
    }
}

/// Check if a hook belongs to a specific project
fn project_matches(hook: &Hook, project_path: &str, workspace: &Workspace) -> bool {
    workspace.projects().iter().any(|project| {
        project.relative_path() == Path::new(project_path) && hook.project() == &**project
    })
}

/// Parse a single selection string into a Selection enum
fn parse_single_selection(input: &str) -> Selection {
    if let Some((project, hook)) = parse_project_hook_syntax(input) {
        return Selection::ProjectHook { project, hook };
    }

    if let Some(hook_id) = input.strip_prefix(':') {
        return Selection::HookId(hook_id.to_string());
    }

    if input.contains('/') {
        let project_path = input.trim_start_matches("./");
        return Selection::ProjectPath(project_path.to_string());
    }

    // Ambiguous case: could be a hook or project
    // Prioritize hooks over projects for backward compatibility
    Selection::HookId(input.to_string())
}

/// Parse project:hook syntax from a string
fn parse_project_hook_syntax(input: &str) -> Option<(String, String)> {
    input
        .split_once(':')
        .filter(|(_, hook)| !hook.is_empty())
        .map(|(project, hook)| (project.to_string(), hook.to_string()))
}

/// Get skip values from CLI args and environment variables
pub(crate) fn get_skips(cli_skip: &[String]) -> Vec<String> {
    use constants::env_vars::EnvVars;

    // If command line skip values are provided, use only those (overwrite env vars)
    if !cli_skip.is_empty() {
        return cli_skip.to_vec();
    }

    let mut skips = Vec::new();

    // Add SKIP environment variable values
    if let Some(s) = EnvVars::var_os(EnvVars::SKIP) {
        if !s.is_empty() {
            skips.extend(parse_comma_separated(&s.to_string_lossy()));
        }
    }

    // Add PREK_SKIP environment variable values
    if let Some(s) = EnvVars::var_os(EnvVars::PREK_SKIP) {
        if !s.is_empty() {
            skips.extend(parse_comma_separated(&s.to_string_lossy()));
        }
    }

    skips
}

/// Parse comma-separated values, trimming whitespace and filtering empty strings
fn parse_comma_separated(input: &str) -> impl Iterator<Item = String> + '_ {
    input
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}
