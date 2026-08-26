use std::collections::BTreeMap;
use std::fmt::Display;

use serde::de::Error as _;
use serde::{Deserialize, Deserializer};

use super::Error;

fn validate_name(name: &str) -> Result<(), &'static str> {
    if name.is_empty() {
        Err("cannot be empty")
    } else if name.chars().any(char::is_whitespace) {
        Err("cannot contain whitespace")
    } else {
        Ok(())
    }
}

pub(crate) fn validate_group_name(name: &str) -> Result<(), &'static str> {
    validate_name(name)?;
    if name.starts_with('@') {
        Err("uses the reserved `@` prefix")
    } else {
        Ok(())
    }
}

#[derive(Clone, Eq, Ord, PartialEq, PartialOrd)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub(crate) struct PriorityAlias(
    #[cfg_attr(feature = "schemars", schemars(regex(pattern = r"^\S+$")))] String,
);

impl TryFrom<String> for PriorityAlias {
    type Error = String;

    fn try_from(alias: String) -> Result<Self, Self::Error> {
        validate_name(&alias).map_err(|reason| priority_alias_error(&alias, reason))?;
        Ok(Self(alias))
    }
}

impl<'de> Deserialize<'de> for PriorityAlias {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::try_from(String::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

impl std::fmt::Debug for PriorityAlias {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(&self.0, f)
    }
}

impl Display for PriorityAlias {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "schemars", serde(untagged))]
pub(crate) enum Priority {
    Number(u32),
    Alias(PriorityAlias),
}

impl<'de> Deserialize<'de> for Priority {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum PriorityWire {
            Number(u32),
            Alias(String),
        }

        match PriorityWire::deserialize(deserializer)? {
            PriorityWire::Number(priority) => Ok(Self::Number(priority)),
            PriorityWire::Alias(alias) => PriorityAlias::try_from(alias)
                .map(Self::Alias)
                .map_err(D::Error::custom),
        }
    }
}

impl std::fmt::Debug for Priority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Number(priority) => std::fmt::Debug::fmt(priority, f),
            Self::Alias(alias) => std::fmt::Debug::fmt(alias, f),
        }
    }
}

impl Priority {
    pub(crate) fn resolve(
        &self,
        priorities: &BTreeMap<PriorityAlias, u32>,
        hook: &str,
    ) -> Result<u32, Error> {
        match self {
            Self::Number(priority) => Ok(*priority),
            Self::Alias(alias) => {
                priorities
                    .get(alias)
                    .copied()
                    .ok_or_else(|| Error::UnknownPriorityAlias {
                        hook: hook.to_owned(),
                        alias: alias.clone(),
                    })
            }
        }
    }
}

fn priority_alias_error(alias: &str, reason: &str) -> String {
    format!("priority alias `{alias}` {reason}")
}

pub(super) fn deserialize_groups<'de, D>(deserializer: D) -> Result<Option<Vec<String>>, D::Error>
where
    D: Deserializer<'de>,
{
    let groups = Option::<Vec<String>>::deserialize(deserializer)?;
    if let Some(groups) = &groups {
        for group in groups {
            if let Err(reason) = validate_group_name(group) {
                return Err(D::Error::custom(format!("group name `{group}` {reason}")));
            }
        }
    }
    Ok(groups)
}
