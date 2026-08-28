use std::collections::BTreeMap;
use std::fmt::Display;
use std::slice;

use anyhow::Result;
use serde::de::{Error as DeError, MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};

use super::hook::{BuiltinHook, HookOptions, LocalHook, MetaHook, RemoteHook};
use super::priority::Priority;

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct RemoteRepo {
    repo: String,
    #[serde(skip)]
    resolved_source: Option<String>,
    pub rev: String,
    #[serde(skip_serializing)]
    pub hooks: Vec<RemoteHook>,

    #[serde(skip_serializing, flatten)]
    pub(super) _unused_keys: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct RemoteRepoKey<'a> {
    source: &'a str,
    rev: &'a str,
}

impl<'a> RemoteRepoKey<'a> {
    pub(crate) fn source(self) -> &'a str {
        self.source
    }

    pub(crate) fn rev(self) -> &'a str {
        self.rev
    }
}

impl RemoteRepo {
    pub fn new(repo: String, rev: String, hooks: Vec<RemoteHook>) -> Self {
        Self {
            repo,
            resolved_source: None,
            rev,
            hooks,
            _unused_keys: BTreeMap::new(),
        }
    }

    /// The repository value exactly as written in the configuration.
    pub(crate) fn repo(&self) -> &str {
        &self.repo
    }

    /// The repository source used for fetch and cache identity.
    pub(crate) fn source(&self) -> &str {
        self.resolved_source.as_deref().unwrap_or(&self.repo)
    }

    pub(crate) fn set_resolved_source(&mut self, source: String) {
        self.resolved_source = Some(source);
    }

    pub fn key(&self) -> RemoteRepoKey<'_> {
        RemoteRepoKey {
            source: self.source(),
            rev: &self.rev,
        }
    }
}

impl std::fmt::Debug for RemoteRepo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut debug = f.debug_struct("RemoteRepo");
        debug.field("repo", &self.repo);
        if let Some(source) = &self.resolved_source {
            debug.field("source", source);
        }
        debug
            .field("rev", &self.rev)
            .field("hooks", &self.hooks)
            .field("_unused_keys", &self._unused_keys)
            .finish()
    }
}

impl Display for RemoteRepo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}@{}", self.repo(), self.rev)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct LocalRepo {
    pub repo: String,
    pub hooks: Vec<LocalHook>,

    #[serde(skip_serializing, flatten)]
    pub(super) _unused_keys: BTreeMap<String, serde_json::Value>,
}

impl Display for LocalRepo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("local")
    }
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct MetaRepo {
    pub repo: String,
    pub hooks: Vec<MetaHook>,

    #[serde(skip_serializing, flatten)]
    pub(super) _unused_keys: BTreeMap<String, serde_json::Value>,
}

impl Display for MetaRepo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("meta")
    }
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct BuiltinRepo {
    pub repo: String,
    pub hooks: Vec<BuiltinHook>,

    #[serde(skip_serializing, flatten)]
    pub(super) _unused_keys: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone)]
pub(crate) enum Repo {
    Remote(RemoteRepo),
    Local(LocalRepo),
    Meta(MetaRepo),
    Builtin(BuiltinRepo),
}

/// A borrowed view of common fields on a hook in a project configuration.
pub(crate) struct HookConfigRef<'a> {
    pub(crate) id: &'a str,
    /// The hook name available without resolving a remote manifest.
    pub(crate) name: Option<&'a str>,
    pub(crate) priority: Option<&'a Priority>,
    pub(crate) options: &'a HookOptions,
}

impl<'a> From<&'a RemoteHook> for HookConfigRef<'a> {
    fn from(hook: &'a RemoteHook) -> Self {
        Self {
            id: &hook.id,
            name: hook.name.as_deref(),
            priority: hook.priority.as_ref(),
            options: &hook.options,
        }
    }
}

impl<'a> From<&'a LocalHook> for HookConfigRef<'a> {
    fn from(hook: &'a LocalHook) -> Self {
        Self {
            id: &hook.id,
            name: Some(hook.name.as_str()),
            priority: hook.priority.as_ref(),
            options: &hook.options,
        }
    }
}

impl<'a> From<&'a MetaHook> for HookConfigRef<'a> {
    fn from(hook: &'a MetaHook) -> Self {
        Self {
            id: &hook.id,
            name: Some(hook.name.as_str()),
            priority: hook.priority.as_ref(),
            options: &hook.options,
        }
    }
}

impl<'a> From<&'a BuiltinHook> for HookConfigRef<'a> {
    fn from(hook: &'a BuiltinHook) -> Self {
        Self {
            id: &hook.id,
            name: Some(hook.name.as_str()),
            priority: hook.priority.as_ref(),
            options: &hook.options,
        }
    }
}

enum RepoHooks<'a> {
    Remote(slice::Iter<'a, RemoteHook>),
    Local(slice::Iter<'a, LocalHook>),
    Meta(slice::Iter<'a, MetaHook>),
    Builtin(slice::Iter<'a, BuiltinHook>),
}

impl<'a> Iterator for RepoHooks<'a> {
    type Item = HookConfigRef<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Remote(hooks) => hooks.next().map(HookConfigRef::from),
            Self::Local(hooks) => hooks.next().map(HookConfigRef::from),
            Self::Meta(hooks) => hooks.next().map(HookConfigRef::from),
            Self::Builtin(hooks) => hooks.next().map(HookConfigRef::from),
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        match self {
            Self::Remote(hooks) => hooks.size_hint(),
            Self::Local(hooks) => hooks.size_hint(),
            Self::Meta(hooks) => hooks.size_hint(),
            Self::Builtin(hooks) => hooks.size_hint(),
        }
    }
}

impl Repo {
    pub(crate) fn hooks(&self) -> impl Iterator<Item = HookConfigRef<'_>> {
        match self {
            Self::Remote(repo) => RepoHooks::Remote(repo.hooks.iter()),
            Self::Local(repo) => RepoHooks::Local(repo.hooks.iter()),
            Self::Meta(repo) => RepoHooks::Meta(repo.hooks.iter()),
            Self::Builtin(repo) => RepoHooks::Builtin(repo.hooks.iter()),
        }
    }
}

impl<'de> Deserialize<'de> for Repo {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct RepoVisitor;

        impl<'de> Visitor<'de> for RepoVisitor {
            type Value = Repo;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a repo mapping")
            }

            fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
            where
                M: MapAccess<'de>,
            {
                enum HooksValue {
                    Remote(Vec<RemoteHook>),
                    Local(Vec<LocalHook>),
                    Meta(Vec<MetaHook>),
                    Builtin(Vec<BuiltinHook>),
                }

                let mut repo: Option<String> = None;
                let mut rev: Option<String> = None;
                let mut hooks: Option<HooksValue> = None;
                let mut unused = BTreeMap::new();

                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "repo" => {
                            let repo_value: String = map.next_value()?;
                            repo = Some(repo_value);
                        }
                        "rev" => {
                            rev = Some(map.next_value()?);
                        }
                        "hooks" => {
                            hooks = Some(match repo.as_deref() {
                                Some("local") => HooksValue::Local(map.next_value()?),
                                Some("meta") => HooksValue::Meta(map.next_value()?),
                                Some("builtin") => HooksValue::Builtin(map.next_value()?),
                                // Not seen `repo` yet, assume remote.
                                _ => HooksValue::Remote(map.next_value()?),
                            });
                        }
                        _ => {
                            let value = map.next_value::<serde_json::Value>()?;
                            unused.insert(key, value);
                        }
                    }
                }

                let repo_value = repo.ok_or_else(|| M::Error::missing_field("repo"))?;
                match repo_value.as_str() {
                    "local" => {
                        if rev.is_some() {
                            return Err(M::Error::custom("`rev` is not allowed for local repos"));
                        }
                        let hooks = match hooks.ok_or_else(|| M::Error::missing_field("hooks"))? {
                            HooksValue::Local(hooks) => hooks,
                            HooksValue::Remote(hooks) => hooks
                                .into_iter()
                                .map(|hook| LocalHook::try_from(hook).map_err(M::Error::custom))
                                .collect::<Result<Vec<_>, _>>()?,
                            HooksValue::Meta(_) | HooksValue::Builtin(_) => {
                                return Err(M::Error::custom("invalid hooks for local repo"));
                            }
                        };
                        Ok(Repo::Local(LocalRepo {
                            repo: "local".to_string(),
                            hooks,
                            _unused_keys: unused,
                        }))
                    }
                    "meta" => {
                        if rev.is_some() {
                            return Err(M::Error::custom("`rev` is not allowed for meta repos"));
                        }
                        let hooks = match hooks.ok_or_else(|| M::Error::missing_field("hooks"))? {
                            HooksValue::Meta(hooks) => hooks,
                            HooksValue::Remote(hooks) => hooks
                                .into_iter()
                                .map(|hook| MetaHook::try_from(hook).map_err(M::Error::custom))
                                .collect::<Result<Vec<_>, _>>()?,
                            HooksValue::Local(_) | HooksValue::Builtin(_) => {
                                return Err(M::Error::custom("invalid hooks for meta repo"));
                            }
                        };
                        Ok(Repo::Meta(MetaRepo {
                            repo: "meta".to_string(),
                            hooks,
                            _unused_keys: unused,
                        }))
                    }
                    "builtin" => {
                        if rev.is_some() {
                            return Err(M::Error::custom("`rev` is not allowed for builtin repos"));
                        }
                        let hooks = match hooks.ok_or_else(|| M::Error::missing_field("hooks"))? {
                            HooksValue::Builtin(hooks) => hooks,
                            HooksValue::Remote(hooks) => hooks
                                .into_iter()
                                .map(|hook| BuiltinHook::try_from(hook).map_err(M::Error::custom))
                                .collect::<Result<Vec<_>, _>>()?,
                            HooksValue::Local(_) | HooksValue::Meta(_) => {
                                return Err(M::Error::custom("invalid hooks for builtin repo"));
                            }
                        };
                        Ok(Repo::Builtin(BuiltinRepo {
                            repo: "builtin".to_string(),
                            hooks,
                            _unused_keys: unused,
                        }))
                    }
                    _ => {
                        let rev = rev.ok_or_else(|| M::Error::missing_field("rev"))?;
                        let hooks = match hooks.ok_or_else(|| M::Error::missing_field("hooks"))? {
                            HooksValue::Remote(hooks) => hooks,
                            HooksValue::Local(_) | HooksValue::Meta(_) | HooksValue::Builtin(_) => {
                                return Err(M::Error::custom("invalid hooks for remote repo"));
                            }
                        };
                        Ok(Repo::Remote(RemoteRepo {
                            repo: repo_value,
                            resolved_source: None,
                            rev,
                            hooks,
                            _unused_keys: unused,
                        }))
                    }
                }
            }
        }

        deserializer.deserialize_map(RepoVisitor)
    }
}
