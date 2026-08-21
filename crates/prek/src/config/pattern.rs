use std::fmt::Display;
use std::path::Path;

use anyhow::Result;
use fancy_regex::{BytesMode, Regex, RegexBuilder};
use globset::{Glob, GlobSet};
use itertools::Itertools;
use serde::de::{DeserializeSeed, Error as DeError, MapAccess, Visitor};
use serde::{Deserialize, Deserializer};

#[derive(Clone)]
pub(crate) struct GlobPatterns {
    patterns: Vec<Glob>,
    set: GlobSet,
}

impl GlobPatterns {
    pub(crate) fn new(patterns: Vec<String>) -> Result<Self, globset::Error> {
        let patterns = patterns
            .into_iter()
            .map(|pattern| pattern.parse())
            .collect::<Result<Vec<_>, _>>()?;
        Self::from_globs(patterns)
    }

    pub(crate) fn from_globs(patterns: Vec<Glob>) -> Result<Self, globset::Error> {
        let mut builder = GlobSet::builder();
        for pattern in &patterns {
            builder.add(pattern.clone());
        }
        let set = builder.build()?;
        Ok(Self { patterns, set })
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }

    pub(crate) fn is_match(&self, path: &Path) -> bool {
        self.set.is_match(path)
    }
}

pub(super) fn debug_globs(globs: &[Glob]) -> impl std::fmt::Debug + '_ {
    std::fmt::from_fn(|f| {
        f.debug_list()
            .entries(globs.iter().map(Glob::glob))
            .finish()
    })
}

impl std::fmt::Debug for GlobPatterns {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GlobPatterns")
            .field("patterns", &debug_globs(&self.patterns))
            .finish_non_exhaustive()
    }
}

enum FilePatternWire {
    Glob { glob: String },
    GlobList { glob: Vec<String> },
    Regex(String),
}

impl<'de> Deserialize<'de> for FilePatternWire {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct FilePatternVisitor;
        struct GlobFieldVisitor;

        impl<'de> DeserializeSeed<'de> for GlobFieldVisitor {
            type Value = FilePatternWire;

            fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
            where
                D: Deserializer<'de>,
            {
                deserializer.deserialize_any(self)
            }
        }

        impl<'de> Visitor<'de> for GlobFieldVisitor {
            type Value = FilePatternWire;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a string or a list of strings")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: DeError,
            {
                Ok(FilePatternWire::Glob {
                    glob: value.to_owned(),
                })
            }

            fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
            where
                E: DeError,
            {
                Ok(FilePatternWire::Glob { glob: value })
            }

            fn visit_seq<A>(self, seq: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::SeqAccess<'de>,
            {
                Ok(FilePatternWire::GlobList {
                    glob: Deserialize::deserialize(serde::de::value::SeqAccessDeserializer::new(
                        seq,
                    ))?,
                })
            }
        }

        impl<'de> Visitor<'de> for FilePatternVisitor {
            type Value = FilePatternWire;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str(
                    "a regex string or a mapping with `glob` set to a string or list of strings",
                )
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: DeError,
            {
                Ok(FilePatternWire::Regex(value.to_owned()))
            }

            fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
            where
                E: DeError,
            {
                Ok(FilePatternWire::Regex(value))
            }

            fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
            where
                M: MapAccess<'de>,
            {
                let mut glob = None;

                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "glob" => {
                            if glob.is_some() {
                                return Err(M::Error::duplicate_field("glob"));
                            }
                            glob = Some(map.next_value_seed(GlobFieldVisitor)?);
                        }
                        _ => {
                            return Err(M::Error::unknown_field(&key, &["glob"]));
                        }
                    }
                }

                glob.ok_or_else(|| M::Error::missing_field("glob"))
            }
        }

        deserializer.deserialize_any(FilePatternVisitor)
    }
}

#[derive(Debug, thiserror::Error)]
enum FilePatternWireError {
    #[error(transparent)]
    Glob(#[from] globset::Error),

    #[error(transparent)]
    Regex(#[from] fancy_regex::Error),
}

#[derive(Debug, Clone, Deserialize)]
#[serde(try_from = "FilePatternWire")]
pub(crate) enum FilePattern {
    Never,
    Regex(Regex),
    Glob(GlobPatterns),
}

impl FilePattern {
    pub(crate) fn glob(patterns: Vec<String>) -> Result<Self, globset::Error> {
        Ok(Self::Glob(GlobPatterns::new(patterns)?))
    }

    pub(crate) fn regex(pattern: &str) -> Result<Self, fancy_regex::Error> {
        let regex = RegexBuilder::new(pattern)
            .bytes_mode(BytesMode::UnicodeBytes)
            .build()?;
        Ok(Self::Regex(regex))
    }

    pub(crate) fn is_match(&self, path: &Path) -> bool {
        match self {
            FilePattern::Never => false,
            FilePattern::Regex(regex) => regex
                .is_match(path.as_os_str().as_encoded_bytes())
                .unwrap_or(false),
            FilePattern::Glob(globs) => globs.is_match(path),
        }
    }
}

impl Display for FilePattern {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FilePattern::Never => f.write_str("never"),
            FilePattern::Regex(regex) => write!(f, "regex: {}", regex.as_str()),
            FilePattern::Glob(globs) => {
                let patterns = globs.patterns.iter().join(", ");
                write!(f, "glob: [{patterns}]")
            }
        }
    }
}

impl TryFrom<FilePatternWire> for FilePattern {
    type Error = FilePatternWireError;

    fn try_from(value: FilePatternWire) -> Result<Self, Self::Error> {
        match value {
            FilePatternWire::Glob { glob } => Ok(Self::glob(vec![glob])?),
            FilePatternWire::GlobList { glob } => Ok(Self::glob(glob)?),
            FilePatternWire::Regex(pattern) => Ok(Self::regex(&pattern)?),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::assert_matches;

    use super::*;

    #[test]
    fn parse_file_patterns_regex_and_glob() {
        #[derive(Debug, Deserialize)]
        struct Wrapper {
            files: FilePattern,
            exclude: FilePattern,
        }

        let regex_yaml = indoc::indoc! {r"
            files: ^src/
            exclude: ^target/
        "};
        let parsed: Wrapper =
            serde_saphyr::from_str(regex_yaml).expect("regex patterns should parse");
        assert_matches!(
            parsed.files,
            FilePattern::Regex(_),
            "expected regex pattern"
        );
        assert!(parsed.files.is_match(Path::new("src/main.rs")));
        assert!(!parsed.files.is_match(Path::new("other/main.rs")));
        assert!(parsed.exclude.is_match(Path::new("target/debug/app")));

        let glob_yaml = indoc::indoc! {r"
            files:
              glob: src/**/*.rs
            exclude:
              glob: target/**
        "};
        let parsed: Wrapper =
            serde_saphyr::from_str(glob_yaml).expect("glob patterns should parse");
        assert_matches!(parsed.files, FilePattern::Glob(_), "expected glob pattern");
        assert!(parsed.files.is_match(Path::new("src/lib/main.rs")));
        assert!(!parsed.files.is_match(Path::new("src/lib/main.py")));
        assert!(parsed.exclude.is_match(Path::new("target/debug/app")));
        assert!(!parsed.exclude.is_match(Path::new("src/lib/main.rs")));

        let glob_list_yaml = indoc::indoc! {r"
            files:
              glob:
                - src/**/*.rs
                - crates/**/src/**/*.rs
            exclude:
              glob:
                - target/**
                - dist/**
        "};
        let parsed: Wrapper =
            serde_saphyr::from_str(glob_list_yaml).expect("glob list patterns should parse");
        assert!(parsed.files.is_match(Path::new("src/lib/main.rs")));
        assert!(parsed.files.is_match(Path::new("crates/foo/src/lib.rs")));
        assert!(!parsed.files.is_match(Path::new("tests/main.rs")));
        assert!(parsed.exclude.is_match(Path::new("target/debug/app")));
        assert!(parsed.exclude.is_match(Path::new("dist/app")));
    }

    #[test]
    fn file_patterns_expose_sources_and_display() {
        let pattern: FilePattern = serde_saphyr::from_str(indoc::indoc! {r"
            glob:
              - src/**/*.rs
              - crates/**/src/**/*.rs
        "})
        .expect("glob list should parse");
        assert_eq!(
            pattern.to_string(),
            "glob: [src/**/*.rs, crates/**/src/**/*.rs]"
        );
        assert!(pattern.is_match(Path::new("src/main.rs")));
        assert!(pattern.is_match(Path::new("crates/foo/src/lib.rs")));
        assert!(!pattern.is_match(Path::new("tests/main.rs")));
    }

    #[test]
    fn file_pattern_never_matches() {
        let pattern = FilePattern::Never;
        assert!(!pattern.is_match(Path::new("")));
        assert!(!pattern.is_match(Path::new("foo.txt")));
        assert!(!pattern.is_match(Path::new("nested/path.rs")));
    }

    #[test]
    fn empty_glob_list_matches_nothing() {
        let pattern = serde_saphyr::from_str::<FilePattern>("glob: []").unwrap();
        assert!(!pattern.is_match(Path::new("any/file.rs")));
        assert!(!pattern.is_match(Path::new("")));
    }

    #[test]
    fn invalid_glob_pattern_errors() {
        let err = serde_saphyr::from_str::<FilePattern>("glob: \"[\"")
            .expect_err("invalid glob should fail");
        let msg = err.to_string().to_lowercase();
        assert!(
            msg.contains("glob"),
            "error should mention glob issues: {msg}"
        );
    }
}
