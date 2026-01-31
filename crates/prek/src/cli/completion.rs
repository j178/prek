use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::path::Path;

use clap::builder::StyledStr;
use clap_complete::CompletionCandidate;
use rustc_hash::FxHashSet;

use crate::config;
use crate::fs::CWD;
use crate::store::Store;
use crate::workspace::{Project, Workspace};

/// Provide completion candidates for `include` and `skip` selectors.
pub(crate) fn selector_completer(current: &OsStr) -> Vec<CompletionCandidate> {
    let Some(current_str) = current.to_str() else {
        return vec![];
    };

    let Some(workspace) = discover_workspace() else {
        return vec![];
    };

    let input = SelectorInput::parse(current_str);
    let mut candidates = Vec::new();

    if input.path.contains('/') {
        return complete_path_selector(&input, &workspace);
    }

    // No slash: match subdirectories under cwd and hook ids across workspace.
    candidates.extend(list_subdirs(&CWD, "", input.raw, &workspace));
    // Also suggest immediate child project roots as `name:`.
    candidates.extend(list_direct_project_colons(&CWD, "", input.raw, &workspace));

    // If the input includes a colon, suggest hooks for that project.
    if let Some(hook_prefix) = input.hook_prefix {
        if !input.path.is_empty() {
            let project_dir_abs = CWD.join(Path::new(input.path));
            if let Some(proj) = find_project_by_abs_path(&workspace, &project_dir_abs) {
                push_project_hooks(&mut candidates, proj, input.path, hook_prefix);
            }
        }
    }

    // Aggregate unique hooks and filter by id.
    let mut uniq: BTreeMap<String, Option<String>> = BTreeMap::new();
    for proj in workspace.projects() {
        for (id, name) in iter_hooks(proj) {
            if id.contains(input.raw) {
                uniq.entry(id.to_owned())
                    .or_insert_with(|| name.map(ToOwned::to_owned));
            }
        }
    }
    candidates.extend(
        uniq.into_iter()
            .map(|(id, name)| CompletionCandidate::new(id).help(name.map(StyledStr::from))),
    );

    candidates
}

struct SelectorInput<'a> {
    raw: &'a str,
    path: &'a str,
    hook_prefix: Option<&'a str>,
}

impl<'a> SelectorInput<'a> {
    fn parse(raw: &'a str) -> Self {
        let (path, hook_prefix) = match raw.split_once(':') {
            Some((path, hook_prefix)) => (path, Some(hook_prefix)),
            None => (raw, None),
        };
        Self {
            raw,
            path,
            hook_prefix,
        }
    }
}

fn discover_workspace() -> Option<Workspace> {
    let store = Store::from_settings().ok()?;
    let root = Workspace::find_root(None, &CWD).ok()?;
    Workspace::discover(&store, root, None, None, false).ok()
}

fn find_project_by_abs_path<'a>(
    workspace: &'a Workspace,
    abs: &Path,
) -> Option<&'a std::sync::Arc<Project>> {
    workspace.projects().iter().find(|p| p.path() == abs)
}

enum RepoHooksIter<'a> {
    Remote(std::slice::Iter<'a, config::RemoteHook>),
    Local(std::slice::Iter<'a, config::LocalHook>),
    Meta(std::slice::Iter<'a, config::MetaHook>),
    Builtin(std::slice::Iter<'a, config::BuiltinHook>),
}

impl<'a> Iterator for RepoHooksIter<'a> {
    type Item = (&'a str, Option<&'a str>);

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Remote(it) => it.next().map(|h| (h.id.as_str(), h.name.as_deref())),
            Self::Local(it) => it.next().map(|h| (h.id.as_str(), Some(h.name.as_str()))),
            Self::Meta(it) => it.next().map(|h| (h.id.as_str(), Some(h.name.as_str()))),
            Self::Builtin(it) => it.next().map(|h| (h.id.as_str(), Some(h.name.as_str()))),
        }
    }
}

fn repo_hooks_iter(repo: &config::Repo) -> RepoHooksIter<'_> {
    match repo {
        config::Repo::Remote(cfg) => RepoHooksIter::Remote(cfg.hooks.iter()),
        config::Repo::Local(cfg) => RepoHooksIter::Local(cfg.hooks.iter()),
        config::Repo::Meta(cfg) => RepoHooksIter::Meta(cfg.hooks.iter()),
        config::Repo::Builtin(cfg) => RepoHooksIter::Builtin(cfg.hooks.iter()),
    }
}

fn iter_hooks(proj: &Project) -> impl Iterator<Item = (&str, Option<&str>)> {
    proj.config()
        .repos
        .iter()
        .flat_map(|repo| repo_hooks_iter(repo))
}

fn push_project_hooks(
    candidates: &mut Vec<CompletionCandidate>,
    proj: &Project,
    selector_prefix: &str,
    hook_prefix: &str,
) {
    for (hook_id, hook_name) in iter_hooks(proj) {
        if !hook_prefix.is_empty() && !hook_id.contains(hook_prefix) {
            continue;
        }
        let value = format!("{selector_prefix}:{hook_id}");
        candidates.push(
            CompletionCandidate::new(value)
                .help(hook_name.map(|name| StyledStr::from(name.to_owned()))),
        );
    }
}

fn complete_path_selector(
    input: &SelectorInput<'_>,
    workspace: &Workspace,
) -> Vec<CompletionCandidate> {
    let mut candidates = Vec::new();

    // Provide subdirectory matches relative to cwd for the path prefix.
    let path_obj = Path::new(input.path);
    let (base_dir, shown_prefix, filter_prefix) = if input.path.ends_with('/') {
        (CWD.join(path_obj), input.path.to_string(), String::new())
    } else {
        let parent = path_obj.parent().unwrap_or(Path::new(""));
        let file = path_obj.file_name().and_then(OsStr::to_str).unwrap_or("");
        let shown_prefix = if parent.as_os_str().is_empty() {
            String::new()
        } else {
            format!("{}/", parent.display())
        };
        (CWD.join(parent), shown_prefix, file.to_string())
    };

    let mut had_children = false;
    if input.hook_prefix.is_none() {
        let mut child_dirs = list_subdirs(&base_dir, &shown_prefix, &filter_prefix, workspace);
        let mut child_colons =
            list_direct_project_colons(&base_dir, &shown_prefix, &filter_prefix, workspace);
        had_children = !(child_dirs.is_empty() && child_colons.is_empty());
        candidates.append(&mut child_dirs);
        candidates.append(&mut child_colons);
    }

    let project_dir_abs = if input.path.ends_with('/') {
        CWD.join(input.path.trim_end_matches('/'))
    } else {
        CWD.join(path_obj)
    };

    // If the path refers to a project directory in the workspace and a colon is present,
    // suggest `path:hook_id`. For pure path input (no colon), don't suggest hooks.
    if let Some(hook_prefix) = input.hook_prefix {
        if let Some(proj) = find_project_by_abs_path(workspace, &project_dir_abs) {
            let selector_prefix = input.path.trim_end_matches('/');
            push_project_hooks(&mut candidates, proj, selector_prefix, hook_prefix);
        }
    } else if input.path.ends_with('/') {
        // No colon and trailing slash: if this base dir is a leaf project (no child projects),
        // suggest the directory itself (with trailing '/').
        if find_project_by_abs_path(workspace, &project_dir_abs).is_some() && !had_children {
            candidates.push(CompletionCandidate::new(input.path.to_string()));
        }
    }

    candidates
}

// List subdirectories under base that contain projects (immediate or nested),
// derived solely from workspace discovery; always end with '/'
fn list_subdirs(
    base: &Path,
    shown_prefix: &str,
    filter_prefix: &str,
    workspace: &Workspace,
) -> Vec<CompletionCandidate> {
    let mut out = Vec::new();

    for name in child_project_names(base, workspace) {
        if !filter_prefix.is_empty() && !name.contains(filter_prefix) {
            continue;
        }

        let mut value = String::new();
        value.push_str(shown_prefix);
        value.push_str(&name);
        if !value.ends_with('/') {
            value.push('/');
        }
        out.push(CompletionCandidate::new(value));
    }

    out
}

fn child_project_names(base: &Path, workspace: &Workspace) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for proj in workspace.projects() {
        let p = proj.path();
        if let Ok(rel) = p.strip_prefix(base) {
            if rel.as_os_str().is_empty() {
                continue;
            }
            if let Some(first) = rel.components().next() {
                names.insert(first.as_os_str().to_string_lossy().to_string());
            }
        }
    }
    names
}

// List immediate child directories under `base` that are themselves project roots,
// suggesting them as `name:` (or `shown_prefix + name + :`)
fn list_direct_project_colons(
    base: &Path,
    shown_prefix: &str,
    filter_prefix: &str,
    workspace: &Workspace,
) -> Vec<CompletionCandidate> {
    let mut out = Vec::new();

    // Build a set of absolute project paths for quick lookup.
    let proj_paths: FxHashSet<_> = workspace.projects().iter().map(|p| p.path()).collect();

    for name in child_project_names(base, workspace) {
        if !filter_prefix.is_empty() && !name.contains(filter_prefix) {
            continue;
        }
        let child_abs = base.join(&name);
        if !proj_paths.contains(child_abs.as_path()) {
            continue;
        }

        let mut value = String::new();
        value.push_str(shown_prefix);
        value.push_str(&name);
        value.push(':');
        out.push(CompletionCandidate::new(value));
    }
    out
}
