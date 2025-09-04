use std::borrow::Cow;
use std::fmt::Display;
use std::path::Path;

use constants::env_vars::EnvVars;
use rustc_hash::FxHashSet;

use crate::hook::Hook;
use crate::warn_user;
use crate::workspace::Workspace;

#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error("Invalid selector: `{selector}`: {reason}")]
    InvalidSelector {
        selector: String,
        reason: &'static str,
    },
}

#[derive(Debug, Clone, Copy)]
enum PathMatch<'a> {
    Exact(&'a String),
    Prefix(&'a String),
}

impl PathMatch<'_> {
    fn matches(&self, path: &Path) -> bool {
        match self {
            PathMatch::Exact(p) => path == Path::new(p),
            PathMatch::Prefix(p) => path.starts_with(p),
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum SelectorSource {
    CliArg,
    CliFlag(&'static str),
    EnvVar(&'static str),
}

#[derive(Debug, Clone)]
pub(crate) enum Selector {
    HookId(String),
    ProjectPrefix(String),
    ProjectHook {
        project_path: String,
        hook_id: String,
    },
}

#[derive(Debug, Clone)]
pub(crate) struct Selection {
    source: SelectorSource,
    original: String,
    selector: Selector,
}

impl Display for Selection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.selector {
            Selector::HookId(hook_id) => write!(f, ":{hook_id}"),
            Selector::ProjectPrefix(project_path) => write!(f, "{project_path}/"),
            Selector::ProjectHook {
                project_path,
                hook_id,
            } => write!(f, "{project_path}:{hook_id}"),
        }
    }
}

impl Selection {
    pub(crate) fn as_flag(&self) -> Cow<'_, str> {
        match &self.source {
            SelectorSource::CliArg => Cow::Borrowed(&self.original),
            SelectorSource::CliFlag(flag) => Cow::Owned(format!("{}={}", flag, self.original)),
            SelectorSource::EnvVar(var) => Cow::Owned(format!("{}={}", var, self.original)),
        }
    }
}

impl Selection {
    /// Check if a hook belongs to a specific project
    fn project_matches(hook: &Hook, project_path: PathMatch, workspace: &Workspace) -> bool {
        workspace.projects().iter().any(|project| {
            project_path.matches(project.relative_path()) && hook.project() == &**project
        })
    }

    pub(crate) fn matches_hook(&self, hook: &Hook, workspace: &Workspace) -> bool {
        match &self.selector {
            Selector::HookId(hook_id) => {
                // For bare hook IDs, check if it matches the hook
                hook.id == *hook_id || hook.alias == *hook_id
            }
            Selector::ProjectPrefix(project_path) => {
                // For project paths, check if the hook belongs to that project
                Self::project_matches(hook, PathMatch::Prefix(project_path), workspace)
            }
            Selector::ProjectHook {
                project_path,
                hook_id,
            } => {
                // For project:hook syntax, check both
                (hook.id == *hook_id || hook.alias == *hook_id)
                    && Self::project_matches(hook, PathMatch::Exact(project_path), workspace)
            }
        }
    }

    pub(crate) fn matches_path(&self, path: &Path) -> bool {
        match &self.selector {
            Selector::ProjectPrefix(project_path) => PathMatch::Prefix(project_path).matches(path),
            Selector::ProjectHook { project_path, .. } => {
                PathMatch::Exact(project_path).matches(path)
            }
            Selector::HookId(_) => false,
        }
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
            .map(|selector| parse_single_selection(selector, SelectorSource::CliArg))
            .collect::<Result<_, _>>()?;

        let skips = get_skips(skips)?;

        Ok(Self { selections, skips })
    }

    /// Parse only selectors (with empty skips).
    pub(crate) fn parse(selectors: &[String]) -> Result<Selections, Error> {
        let selections = selectors
            .iter()
            .map(|selector| parse_single_selection(selector, SelectorSource::CliArg))
            .collect::<Result<_, _>>()?;

        Ok(Self {
            selections,
            skips: vec![],
        })
    }

    /// Check if a hook matches any of the selection criteria
    pub(crate) fn matches_hook(
        &self,
        hook: &Hook,
        workspace: &Workspace,
        usage: &mut SelectorUsage,
    ) -> bool {
        if let Some((idx, _)) = self
            .skips
            .iter()
            .enumerate()
            .find(|(_, skip)| skip.matches_hook(hook, workspace))
        {
            usage.use_skip(idx);
            return false;
        }

        if self.selections.is_empty() {
            return true; // No selections mean all hooks are included
        }

        if let Some((idx, _)) = self
            .selections
            .iter()
            .enumerate()
            .find(|(_, selection)| selection.matches_hook(hook, workspace))
        {
            usage.use_selector(idx);
            return true;
        }

        false
    }

    pub(crate) fn matches_path(&self, path: &Path, usage: &mut SelectorUsage) -> bool {
        if let Some((idx, _)) = self
            .skips
            .iter()
            .enumerate()
            .find(|(_, skip)| skip.matches_path(path))
        {
            usage.use_skip(idx);
            return false;
        }

        if self.selections.is_empty() {
            return true; // No selections mean all paths are included
        }

        if let Some((idx, _)) = self
            .selections
            .iter()
            .enumerate()
            .find(|(_, selection)| selection.matches_path(path))
        {
            usage.use_selector(idx);
            return true;
        }

        false
    }
}

#[derive(Default, Debug)]
pub(crate) struct SelectorUsage {
    pub(crate) used_selectors: FxHashSet<usize>,
    pub(crate) used_skips: FxHashSet<usize>,
}

impl SelectorUsage {
    pub(crate) fn use_selector(&mut self, idx: usize) {
        self.used_selectors.insert(idx);
    }

    pub(crate) fn use_skip(&mut self, idx: usize) {
        self.used_skips.insert(idx);
    }

    pub(crate) fn report_unused(&self, selections: &Selections) {
        let unused = selections
            .selections
            .iter()
            .enumerate()
            .filter(|(idx, _)| !self.used_selectors.contains(idx))
            .chain(
                selections
                    .skips
                    .iter()
                    .enumerate()
                    .filter(|(idx, _)| !self.used_skips.contains(idx)),
            )
            .collect::<Vec<_>>();

        match unused.as_slice() {
            [] => {}
            [(_, selection)] => {
                warn_user!(
                    "selector `{}` did not match any hooks or projects",
                    selection.as_flag()
                );
            }
            _ => {
                warn_user!(
                    "the following selectors did not match any hooks or projects:\n{}",
                    unused
                        .iter()
                        .map(|(_, sel)| format!("  - `{}`", sel.as_flag()))
                        .collect::<Vec<_>>()
                        .join("\n")
                );
            }
        }
    }
}

/// Parse a single selection string into a Selection enum
fn parse_single_selection(input: &str, source: SelectorSource) -> Result<Selection, Error> {
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
        return Ok(Selection {
            source,
            original: input.to_string(),
            selector: Selector::HookId(hook_id.to_string()),
        });
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
        let project = project.trim_start_matches(['.', '/']).trim_end_matches('/');
        return Ok(Selection {
            source,
            original: input.to_string(),
            selector: Selector::ProjectHook {
                project_path: project.to_string(),
                hook_id: hook.to_string(),
            },
        });
    }

    // Handle project paths
    if input == "." || input.contains('/') {
        let project_path = input
            .trim_start_matches(['.', '/'])
            .trim_end_matches('/')
            .to_string();
        return Ok(Selection {
            source,
            original: input.to_string(),
            selector: Selector::ProjectPrefix(project_path),
        });
    }

    // Ambiguous case: treat as hook ID for backward compatibility
    if input.is_empty() {
        return Err(Error::InvalidSelector {
            selector: input.to_string(),
            reason: "cannot be empty",
        });
    }
    Ok(Selection {
        source,
        original: input.to_string(),
        selector: Selector::HookId(input.to_string()),
    })
}

/// Parse skip selectors from CLI args and environment variables
pub(crate) fn get_skips(cli_skips: &[String]) -> Result<Vec<Selection>, Error> {
    let (skips, source) = if !cli_skips.is_empty() {
        (cli_skips.to_vec(), SelectorSource::CliFlag("--skip"))
    } else if let Ok(s) = EnvVars::var(EnvVars::PREK_SKIP) {
        (
            parse_comma_separated(&s).collect(),
            SelectorSource::EnvVar(EnvVars::PREK_SKIP),
        )
    } else if let Ok(s) = EnvVars::var(EnvVars::SKIP) {
        (
            parse_comma_separated(&s).collect(),
            SelectorSource::EnvVar(EnvVars::SKIP),
        )
    } else {
        return Ok(vec![]);
    };

    skips
        .iter()
        .map(|skip| parse_single_selection(skip, source))
        .collect()
}

/// Parse comma-separated values, trimming whitespace and filtering empty strings
fn parse_comma_separated(input: &str) -> impl Iterator<Item = String> {
    input
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
}
