use clap::builder::StyledStr;
use clap_complete::CompletionCandidate;

use crate::config;
use crate::git::GIT_ROOT;
use crate::workspace::Project;

/// Provide completion candidates for `include` and `skip` selectors.
pub(crate) fn selector_completer(current: &std::ffi::OsStr) -> Vec<CompletionCandidate> {
    get_hook_id_candidates(current).unwrap_or_default()
}

fn get_hook_id_candidates(current: &std::ffi::OsStr) -> anyhow::Result<Vec<CompletionCandidate>> {
    // TODO: find from ancestor directories up to the root of the git repository
    let project = Project::from_directory(GIT_ROOT.as_ref()?)?;

    let hook_ids = project
        .config()
        .repos
        .iter()
        .flat_map(
            |repo| -> Box<dyn Iterator<Item = (&String, Option<&str>)>> {
                match repo {
                    config::Repo::Remote(cfg) => {
                        Box::new(cfg.hooks.iter().map(|h| (&h.id, h.name.as_deref())))
                    }
                    config::Repo::Local(cfg) => {
                        Box::new(cfg.hooks.iter().map(|h| (&h.id, Some(&*h.name))))
                    }
                    config::Repo::Meta(cfg) => {
                        Box::new(cfg.hooks.iter().map(|h| (&h.0.id, Some(&*h.0.name))))
                    }
                }
            },
        )
        .map(|(id, name)| {
            CompletionCandidate::new(id.clone())
                .help(name.map(|name| StyledStr::from(name.to_string())))
        });

    let Some(current) = current.to_str() else {
        return Ok(hook_ids.collect());
    };

    Ok(hook_ids
        .filter(|h| h.get_value().to_str().unwrap_or_default().contains(current))
        .collect())
}
