use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::PathBuf;

use clap::builder::StyledStr;
use clap_complete::CompletionCandidate;

use crate::config;
use crate::fs::{CWD, PathClean};
use crate::store::Store;
use crate::workspace::{Project, Workspace};

pub(crate) fn selector_completer(current: &OsStr) -> Vec<CompletionCandidate> {
    let Some(current) = current.to_str() else {
        return Vec::new();
    };
    let Some(completer) = SelectorCompleter::load() else {
        return Vec::new();
    };

    completer.complete(SelectorQuery::parse(current))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SelectorQuery<'a> {
    Hook { fragment: &'a str },
    Project { path: &'a str },
    ProjectHook { project: &'a str, fragment: &'a str },
    HookOrProject { fragment: &'a str },
}

impl<'a> SelectorQuery<'a> {
    fn parse(current: &'a str) -> Self {
        if let Some(fragment) = current.strip_prefix(':') {
            return Self::Hook { fragment };
        }

        if let Some((project, fragment)) = current.split_once(':') {
            return Self::ProjectHook { project, fragment };
        }

        if current == "." || current.contains('/') {
            return Self::Project { path: current };
        }

        Self::HookOrProject { fragment: current }
    }
}

struct SelectorCompleter {
    workspace: Workspace,
}

impl SelectorCompleter {
    fn load() -> Option<Self> {
        let store = Store::from_settings().ok()?;
        let root = Workspace::find_root(None, &CWD).ok()?;
        let workspace = Workspace::discover(&store, root, None, None, false).ok()?;

        Some(Self { workspace })
    }

    fn complete(&self, query: SelectorQuery<'_>) -> Vec<CompletionCandidate> {
        match query {
            SelectorQuery::Hook { fragment } => hook_candidates(
                self.projects(),
                fragment,
                HookTarget::Workspace { explicit: true },
            ),
            SelectorQuery::Project { path } => self.project_candidates(path),
            SelectorQuery::ProjectHook { project, fragment } => self
                .find_project(project)
                .map(|matched_project| {
                    hook_candidates(
                        std::iter::once(matched_project),
                        fragment,
                        HookTarget::Project(project),
                    )
                })
                .unwrap_or_default(),
            SelectorQuery::HookOrProject { fragment } => {
                let mut candidates = self.project_candidates(fragment);
                candidates.extend(hook_candidates(
                    self.projects(),
                    fragment,
                    HookTarget::Workspace { explicit: false },
                ));
                candidates
            }
        }
    }

    fn projects(&self) -> impl Iterator<Item = &Project> {
        self.workspace.all_projects().iter().map(AsRef::as_ref)
    }

    fn find_project(&self, selector: &str) -> Option<&Project> {
        let path = CWD.join(selector).clean();
        self.projects().find(|project| project.path() == path)
    }

    fn project_candidates(&self, current: &str) -> Vec<CompletionCandidate> {
        if current == "." {
            let mut candidates = vec![CompletionCandidate::new("./")];
            if self.find_project(current).is_some() {
                candidates.push(CompletionCandidate::new(".:"));
            }
            return candidates;
        }

        let query = ProjectPathQuery::parse(current);
        let mut children = BTreeMap::<&OsStr, bool>::new();
        let mut base_is_project = false;
        let mut base_has_projects = false;

        for project in self.projects() {
            let Ok(relative) = project.path().strip_prefix(&query.base) else {
                continue;
            };
            let mut components = relative.components();
            let Some(first) = components.next() else {
                base_is_project = true;
                continue;
            };

            base_has_projects = true;
            let is_direct_project = components.next().is_none();
            children
                .entry(first.as_os_str())
                .and_modify(|direct| *direct |= is_direct_project)
                .or_insert(is_direct_project);
        }

        let mut candidates = Vec::with_capacity(children.len() * 2 + 1);
        for (name, is_project) in children {
            let Some(name) = name.to_str() else {
                continue;
            };
            if !matches_fragment(name, query.fragment) {
                continue;
            }

            let value = format!("{}{name}", query.shown_prefix);
            candidates.push(CompletionCandidate::new(format!("{value}/")));
            if is_project {
                candidates.push(CompletionCandidate::new(format!("{value}:")));
            }
        }

        if query.ends_with_slash && base_is_project && !base_has_projects {
            candidates.push(CompletionCandidate::new(current));
        }

        candidates
    }
}

struct ProjectPathQuery<'a> {
    base: PathBuf,
    shown_prefix: &'a str,
    fragment: &'a str,
    ends_with_slash: bool,
}

impl<'a> ProjectPathQuery<'a> {
    fn parse(current: &'a str) -> Self {
        let ends_with_slash = current.ends_with('/');
        let (shown_prefix, fragment) = if ends_with_slash {
            (current, "")
        } else if let Some((parent, name)) = current.rsplit_once('/') {
            (&current[..=parent.len()], name)
        } else {
            ("", current)
        };

        Self {
            base: CWD.join(shown_prefix).clean(),
            shown_prefix,
            fragment,
            ends_with_slash,
        }
    }
}

fn hook_candidates<'a>(
    projects: impl IntoIterator<Item = &'a Project>,
    fragment: &str,
    target: HookTarget<'_>,
) -> Vec<CompletionCandidate> {
    let mut hooks = BTreeMap::<&str, Option<&str>>::new();

    for project in projects {
        visit_hooks(project, |id, name| {
            if !matches_fragment(id, fragment) {
                return;
            }

            hooks
                .entry(id)
                .and_modify(|current_name| {
                    if current_name.is_none() {
                        *current_name = name;
                    }
                })
                .or_insert(name);
        });
    }

    hooks
        .into_iter()
        .map(|(id, name)| {
            CompletionCandidate::new(target.render(id))
                .help(name.map(|name| StyledStr::from(name.to_owned())))
        })
        .collect()
}

#[derive(Clone, Copy)]
enum HookTarget<'a> {
    Workspace { explicit: bool },
    Project(&'a str),
}

impl HookTarget<'_> {
    fn render(self, id: &str) -> String {
        match self {
            Self::Workspace { explicit } => {
                let explicit = explicit || hook_id_requires_explicit_selector(id);
                let mut value = String::with_capacity(id.len() + usize::from(explicit));
                if explicit {
                    value.push(':');
                }
                value.push_str(id);
                value
            }
            Self::Project(project) => {
                let project = trim_selector_slash(project);
                let mut value = String::with_capacity(project.len() + id.len() + 1);
                value.push_str(project);
                value.push(':');
                value.push_str(id);
                value
            }
        }
    }
}

fn visit_hooks<'a>(project: &'a Project, mut visit: impl FnMut(&'a str, Option<&'a str>)) {
    for repo in &project.config().repos {
        match repo {
            config::Repo::Remote(repo) => {
                for hook in &repo.hooks {
                    visit(&hook.id, hook.name.as_deref());
                }
            }
            config::Repo::Local(repo) => {
                for hook in &repo.hooks {
                    visit(&hook.id, Some(&hook.name));
                }
            }
            config::Repo::Meta(repo) => {
                for hook in &repo.hooks {
                    visit(&hook.id, Some(&hook.name));
                }
            }
            config::Repo::Builtin(repo) => {
                for hook in &repo.hooks {
                    visit(&hook.id, Some(&hook.name));
                }
            }
        }
    }
}

fn matches_fragment(value: &str, fragment: &str) -> bool {
    value.contains(fragment)
}

fn hook_id_requires_explicit_selector(id: &str) -> bool {
    id == "." || id.starts_with('-') || id.contains(':') || id.contains('/')
}

fn trim_selector_slash(selector: &str) -> &str {
    let trimmed = selector.trim_end_matches('/');
    if trimmed.is_empty() && selector.starts_with('/') {
        "/"
    } else {
        trimmed
    }
}

#[cfg(test)]
mod tests {
    use super::{SelectorQuery, hook_id_requires_explicit_selector};

    #[test]
    fn query_without_separator_completes_hooks_and_projects() {
        assert_eq!(
            SelectorQuery::parse("ruff"),
            SelectorQuery::HookOrProject { fragment: "ruff" }
        );
    }

    #[test]
    fn query_with_slash_completes_projects() {
        assert_eq!(
            SelectorQuery::parse("apps/api"),
            SelectorQuery::Project { path: "apps/api" }
        );
    }

    #[test]
    fn query_with_project_and_colon_completes_project_hooks() {
        assert_eq!(
            SelectorQuery::parse("apps/api:ruff"),
            SelectorQuery::ProjectHook {
                project: "apps/api",
                fragment: "ruff",
            }
        );
    }

    #[test]
    fn query_with_leading_colon_completes_explicit_hook_ids() {
        assert_eq!(
            SelectorQuery::parse(":lint:ruff"),
            SelectorQuery::Hook {
                fragment: "lint:ruff"
            }
        );
    }

    #[test]
    fn reserved_hook_ids_require_explicit_selectors() {
        assert!(
            ["lint:ruff", "lint/ruff", ".", "--help"]
                .into_iter()
                .all(hook_id_requires_explicit_selector)
        );
    }

    #[test]
    fn regular_hook_ids_use_minimal_selectors() {
        assert!(!hook_id_requires_explicit_selector("ruff-check"));
    }
}
