//! Parallel directory discovery for prek. See [`walk`] for its traversal contract.

#![warn(dead_code)]

use std::path::{Path, PathBuf};

pub use walk::{WalkState, walk};

mod dir;
mod gitignore;
mod pathutil;
mod walk;

#[derive(Debug)]
enum Error {
    Partial(Vec<Self>),
    WithLineNumber { line: u64, err: Box<Self> },
    WithPath { path: PathBuf, err: Box<Self> },
    Io(std::io::Error),
    Glob { glob: Option<String>, err: String },
}

impl Error {
    fn is_io(&self) -> bool {
        match self {
            Self::Partial(errors) => errors.len() == 1 && errors[0].is_io(),
            Self::WithLineNumber { err, .. } | Self::WithPath { err, .. } => err.is_io(),
            Self::Io(_) => true,
            Self::Glob { .. } => false,
        }
    }

    fn with_path(self, path: impl AsRef<Path>) -> Self {
        Self::WithPath {
            path: path.as_ref().to_path_buf(),
            err: Box::new(self),
        }
    }

    fn tagged(self, path: impl AsRef<Path>, line: u64) -> Self {
        let error = Self::WithLineNumber {
            line,
            err: Box::new(self),
        };
        if path.as_ref().as_os_str().is_empty() {
            error
        } else {
            error.with_path(path)
        }
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Partial(errors) => {
                for (index, error) in errors.iter().enumerate() {
                    if index > 0 {
                        writeln!(f)?;
                    }
                    error.fmt(f)?;
                }
                Ok(())
            }
            Self::WithLineNumber { line, err } => write!(f, "line {line}: {err}"),
            Self::WithPath { path, err } => write!(f, "{}: {err}", path.display()),
            Self::Io(err) => err.fmt(f),
            Self::Glob { glob: None, err } => err.fmt(f),
            Self::Glob {
                glob: Some(glob),
                err,
            } => write!(f, "error parsing glob '{glob}': {err}"),
        }
    }
}

#[derive(Default)]
struct PartialErrorBuilder(Vec<Error>);

impl PartialErrorBuilder {
    fn push(&mut self, error: Error) {
        self.0.push(error);
    }

    fn maybe_push_ignore_io(&mut self, error: Option<Error>) {
        if let Some(error) = error {
            if !error.is_io() {
                self.push(error);
            }
        }
    }

    fn into_error_option(mut self) -> Option<Error> {
        match self.0.len() {
            0 => None,
            1 => self.0.pop(),
            _ => Some(Error::Partial(self.0)),
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Match {
    None,
    Ignore,
    Whitelist,
}

impl Match {
    fn is_none(self) -> bool {
        matches!(self, Self::None)
    }
    fn is_ignore(self) -> bool {
        matches!(self, Self::Ignore)
    }
    fn or(self, other: Self) -> Self {
        if self.is_none() { other } else { self }
    }
}
