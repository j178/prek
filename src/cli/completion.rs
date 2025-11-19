use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::path::Path;

use clap::builder::StyledStr;
use clap_complete::CompletionCandidate;

use crate::config;
use crate::fs::CWD;
use crate::store::Store;
use crate::workspace::{Project, Workspace};

/// Provide completion candidates for `include` and `skip` selectors.
pub(crate) fn selector_completer(current: &OsStr) -> Vec<CompletionCandidate> {
    let Some(current_str) = current.to_str() else {
        return vec![];
    };

    let Ok(store) = Store::from_settings() else {
        return vec![];
    };
    let Ok(workspace) = Workspace::find_root(None, &CWD)
        .and_then(|root| Workspace::discover(&store, root, None, None, false))
    else {
        return vec![];
    };

    let mut candidates: Vec<CompletionCandidate> = vec![];

    // Support optional `path:hook_prefix` form while typing.
    let (path_part, hook_prefix_opt) = match current_str.split_once(':') {
        Some((p, rest)) => (p, Some(rest)),
        None => (current_str, None),
    };

    if path_part.contains('/') {
        // Provide subdirectory matches relative to cwd for the path prefix
        let path_obj = Path::new(path_part);
        let (base_dir, shown_prefix, filter_prefix) = if path_part.ends_with('/') {
            (CWD.join(path_obj), path_part.to_string(), String::new())
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
        if hook_prefix_opt.is_none() {
            let mut child_dirs = list_subdirs(&base_dir, &shown_prefix, &filter_prefix, &workspace);
            let mut child_colons =
                list_direct_project_colons(&base_dir, &shown_prefix, &filter_prefix, &workspace);
            had_children = !(child_dirs.is_empty() && child_colons.is_empty());
            candidates.append(&mut child_dirs);
            candidates.append(&mut child_colons);
        }

        // If the path refers to a project directory in the workspace and a colon is present,
        // suggest `path:hook_id`. For pure path input (no colon), don't suggest hooks.
        let project_dir_abs = if path_part.ends_with('/') {
            CWD.join(path_part.trim_end_matches('/'))
        } else {
            CWD.join(path_obj)
        };
        if hook_prefix_opt.is_some() {
            if let Some(proj) = workspace
                .projects()
                .iter()
                .find(|p| p.path() == project_dir_abs)
            {
                let hook_pairs = all_hooks(proj);
                let path_prefix_display = if path_part.ends_with('/') {
                    path_part.trim_end_matches('/')
                } else {
                    path_part
                };
                for (hid, name) in hook_pairs {
                    if let Some(hpref) = hook_prefix_opt {
                        if !hid.starts_with(hpref) && !hid.contains(hpref) {
                            continue;
                        }
                    }
                    let value = format!("{path_prefix_display}:{hid}");
                    candidates
                        .push(CompletionCandidate::new(value).help(name.map(StyledStr::from)));
                }
            }
        } else if path_part.ends_with('/') {
            // No colon and trailing slash: if this base dir is a leaf project (no child projects),
            // suggest the directory itself (with trailing '/').
            let is_project = workspace
                .projects()
                .iter()
                .any(|p| p.path() == project_dir_abs);
            if is_project && !had_children {
                candidates.push(CompletionCandidate::new(path_part.to_string()));
            }
        }

        return candidates;
    }

    // No slash: match subdirectories under cwd and hook ids across workspace
    candidates.extend(list_subdirs(&CWD, "", current_str, &workspace));
    // Also suggest immediate child project roots as `name:`
    candidates.extend(list_direct_project_colons(
        &CWD,
        "",
        current_str,
        &workspace,
    ));

    // If the input ends with `:`, suggest hooks for that project
    if let Some(hook_prefix) = hook_prefix_opt {
        if !path_part.is_empty() {
            let project_dir_abs = CWD.join(Path::new(path_part));
            if let Some(proj) = workspace
                .projects()
                .iter()
                .find(|p| p.path() == project_dir_abs)
            {
                for (hid, name) in all_hooks(proj) {
                    if !hook_prefix.is_empty()
                        && !hid.starts_with(hook_prefix)
                        && !hid.contains(hook_prefix)
                    {
                        continue;
                    }
                    let value = format!("{path_part}:{hid}");
                    candidates
                        .push(CompletionCandidate::new(value).help(name.map(StyledStr::from)));
                }
            }
        }
    }

    // Aggregate unique hooks and filter by id
    let mut uniq: BTreeMap<String, Option<String>> = BTreeMap::new();
    for proj in workspace.projects() {
        for (id, name) in all_hooks(proj) {
            if id.contains(current_str) || id.starts_with(current_str) {
                uniq.entry(id).or_insert(name);
            }
        }
    }
    candidates.extend(
        uniq.into_iter()
            .map(|(id, name)| CompletionCandidate::new(id).help(name.map(StyledStr::from))),
    );

    candidates
}

fn all_hooks(proj: &Project) -> Vec<(String, Option<String>)> {
    let mut out = Vec::new();
    for repo in &proj.config().repos {
        match repo {
            config::Repo::Remote(cfg) => {
                for h in &cfg.hooks {
                    out.push((h.id.clone(), h.name.as_ref().map(ToString::to_string)));
                }
            }
            config::Repo::Local(cfg) => {
                for h in &cfg.hooks {
                    out.push((h.id.clone(), Some(h.name.clone())));
                }
            }
            config::Repo::Meta(cfg) => {
                for h in &cfg.hooks {
                    out.push((h.0.id.clone(), Some(h.0.name.clone())));
                }
            }
            config::Repo::Builtin(cfg) => {
                for h in &cfg.hooks {
                    out.push((h.0.id.clone(), Some(h.0.name.clone())));
                }
            }
        }
    }
    out
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
    let mut first_components: BTreeSet<String> = BTreeSet::new();
    for proj in workspace.projects() {
        let p = proj.path();
        if let Ok(rel) = p.strip_prefix(base) {
            if rel.as_os_str().is_empty() {
                // Project is exactly at base; doesn't yield a child directory
                continue;
            }
            if let Some(first) = rel.components().next() {
                let name = first.as_os_str().to_string_lossy().to_string();
                first_components.insert(name);
            }
        }
    }
    for name in first_components {
        if filter_prefix.is_empty()
            || name.starts_with(filter_prefix)
            || name.contains(filter_prefix)
        {
            let mut value = String::new();
            value.push_str(shown_prefix);
            value.push_str(&name);
            if !value.ends_with('/') {
                value.push('/');
            }
            out.push(CompletionCandidate::new(value));
        }
    }

    out
}

// List immediate child directories under `base` that are themselves project roots,
// suggesting them as `name:` (or `shown_prefix + name + :`)
fn list_direct_project_colons(
    base: &Path,
    shown_prefix: &str,
    filter_prefix: &str,
    workspace: &Workspace,
) -> Vec<CompletionCandidate> {
    // Build a set of absolute project paths for quick lookup
    let proj_paths: BTreeSet<_> = workspace
        .projects()
        .iter()
        .map(|p| p.path().to_path_buf())
        .collect();

    // Compute immediate child names that lead to at least one project (same logic as list_subdirs)
    // then keep only those where `base/child` is itself a project root.
    let mut names: BTreeSet<String> = BTreeSet::new();
    for proj in workspace.projects() {
        let p = proj.path();
        if let Ok(rel) = p.strip_prefix(base) {
            if rel.as_os_str().is_empty() {
                continue;
            }
            if let Some(first) = rel.components().next() {
                let name = first.as_os_str().to_string_lossy().to_string();
                // Only keep if this immediate child is a project root
                let child_abs = base.join(&name);
                if proj_paths.contains(&child_abs) {
                    names.insert(name);
                }
            }
        }
    }

    let mut out = Vec::new();
    for name in names {
        if filter_prefix.is_empty()
            || name.starts_with(filter_prefix)
            || name.contains(filter_prefix)
        {
            let mut value = String::new();
            value.push_str(shown_prefix);
            value.push_str(&name);
            value.push(':');
            out.push(CompletionCandidate::new(value));
        }
    }
    out
}
