use std::collections::BTreeMap;

use anyhow::Result;
use globset::Glob;
use serde::de::Error as _;
use serde::{Deserialize, Deserializer};

use super::pattern::debug_globs;

/// A configuration value that accepts either one string or a list of strings.
#[derive(Clone, Eq, PartialEq)]
pub(crate) enum StringOrList {
    One(Glob),
    Many(Vec<Glob>),
}

impl std::fmt::Debug for StringOrList {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::One(pattern) => f.debug_tuple("One").field(&pattern.glob()).finish(),
            Self::Many(patterns) => f.debug_tuple("Many").field(&debug_globs(patterns)).finish(),
        }
    }
}

impl<'de> Deserialize<'de> for StringOrList {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum RawStringOrList {
            One(String),
            Many(Vec<String>),
        }

        match RawStringOrList::deserialize(deserializer)? {
            RawStringOrList::One(pattern) => {
                pattern.parse().map(Self::One).map_err(D::Error::custom)
            }
            RawStringOrList::Many(patterns) => patterns
                .into_iter()
                .map(|pattern| pattern.parse().map_err(D::Error::custom))
                .collect::<Result<_, _>>()
                .map(Self::Many),
        }
    }
}

impl StringOrList {
    pub(crate) fn as_slice(&self) -> &[Glob] {
        match self {
            Self::One(value) => std::slice::from_ref(value),
            Self::Many(values) => values,
        }
    }
}

/// Overrides tag selection for one repository during `prek update`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "snake_case")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub(crate) struct RepoTagFilterOptions {
    pub(crate) include_tags: Option<StringOrList>,
    pub(crate) exclude_tags: Option<StringOrList>,
}

/// Controls how `prek update` selects eligible releases.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "snake_case")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub(crate) struct UpdateOptions {
    pub(crate) cooldown_days: Option<u8>,
    pub(crate) freeze: Option<bool>,
    pub(crate) include_tags: Option<StringOrList>,
    pub(crate) exclude_tags: Option<StringOrList>,
    pub(crate) repos: BTreeMap<String, RepoTagFilterOptions>,
}
