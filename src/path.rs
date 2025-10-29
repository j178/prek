// Adapted from `[pathclean](https://crates.io/crates/pathclean)`.
// Copyright (c) 2018 Dan Reeves
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN
use std::path::{Component, Path, PathBuf};
use std::sync::LazyLock;

use camino::{Utf8Path, Utf8PathBuf};

/// The `PathClean` trait implements a `clean` method.
pub trait PathClean {
    fn clean(&self) -> PathBuf;
}

/// `PathClean` implemented for `Path`
impl PathClean for Path {
    fn clean(&self) -> PathBuf {
        clean(self)
    }
}

/// `PathClean` implemented for `PathBuf`
impl PathClean for PathBuf {
    fn clean(&self) -> PathBuf {
        clean(self)
    }
}

impl PathClean for camino::Utf8Path {
    fn clean(&self) -> PathBuf {
        clean(self)
    }
}

impl PathClean for camino::Utf8PathBuf {
    fn clean(&self) -> PathBuf {
        clean(self)
    }
}

/// The core implementation. It performs the following, lexically:
/// 1. Reduce multiple slashes to a single slash.
/// 2. Eliminate `.` path name elements (the current directory).
/// 3. Eliminate `..` path name elements (the parent directory) and the non-`.` non-`..`, element that precedes them.
/// 4. Eliminate `..` elements that begin a rooted path, that is, replace `/..` by `/` at the beginning of a path.
/// 5. Leave intact `..` elements that begin a non-rooted path.
///
/// If the result of this process is an empty string, return the string `"."`, representing the current directory.
pub fn clean<P>(path: P) -> PathBuf
where
    P: AsRef<Path>,
{
    let mut out = Vec::new();

    for comp in path.as_ref().components() {
        match comp {
            Component::CurDir => (),
            Component::ParentDir => match out.last() {
                Some(Component::RootDir) => (),
                Some(Component::Normal(_)) => {
                    out.pop();
                }
                None | Some(Component::CurDir | Component::ParentDir | Component::Prefix(_)) => {
                    out.push(comp);
                }
            },
            comp => out.push(comp),
        }
    }

    if out.is_empty() {
        PathBuf::from(".")
    } else {
        out.iter().collect()
    }
}

pub(crate) static CWD: LazyLock<Utf8PathBuf> = LazyLock::new(|| {
    Utf8PathBuf::from_path_buf(std::env::current_dir().expect("The current directory must exist"))
        .expect("Current directory path is not valid UTF-8")
});

/// Normalizes a path to use `/` as a separator everywhere, even on platforms
/// that recognize other characters as separators.
#[cfg(unix)]
pub(crate) fn normalize_path(path: PathBuf) -> PathBuf {
    // UNIX only uses /, so we're good.
    path
}

/// Normalizes a path to use `/` as a separator everywhere, even on platforms
/// that recognize other characters as separators.
#[cfg(not(unix))]
pub(crate) fn normalize_path(path: PathBuf) -> PathBuf {
    use std::ffi::OsString;
    use std::path::is_separator;

    let mut path = path.into_os_string().into_encoded_bytes();
    for c in &mut path {
        if *c == b'/' || !is_separator(char::from(*c)) {
            continue;
        }
        *c = b'/';
    }

    let os_str = OsString::from(String::from_utf8_lossy(&path).to_string());
    PathBuf::from(os_str)
}

/// Compute a path describing `path` relative to `base`.
///
/// `lib/python/site-packages/foo/__init__.py` and `lib/python/site-packages` -> `foo/__init__.py`
/// `lib/marker.txt` and `lib/python/site-packages` -> `../../marker.txt`
/// `bin/foo_launcher` and `lib/python/site-packages` -> `../../../bin/foo_launcher`
///
/// Returns `Err` if there is no relative path between `path` and `base` (for example, if the paths
/// are on different drives on Windows).
pub fn relative_to(
    path: impl AsRef<Path>,
    base: impl AsRef<Path>,
) -> Result<Utf8PathBuf, std::io::Error> {
    // Find the longest common prefix, and also return the path stripped from that prefix
    let (stripped, common_prefix) = base
        .as_ref()
        .ancestors()
        .find_map(|ancestor| {
            // Simplifying removes the UNC path prefix on windows.
            dunce::simplified(path.as_ref())
                .strip_prefix(dunce::simplified(ancestor))
                .ok()
                .map(|stripped| (stripped, ancestor))
        })
        .ok_or_else(|| {
            std::io::Error::other(format!(
                "Trivial strip failed: {} vs. {}",
                path.as_ref().display(),
                base.as_ref().display()
            ))
        })?;

    // go as many levels up as required
    let levels_up = base.as_ref().components().count() - common_prefix.components().count();
    let up = std::iter::repeat_n("..", levels_up).collect::<PathBuf>();

    Ok(up.join(stripped).into_utf8_path_buf())
}

pub trait ToUtf8Path {
    /// Convert a [`Path`] to a [`Utf8Path`], panicking if the path is not valid UTF-8.
    fn to_utf8_path(&self) -> &Utf8Path;
}

pub trait IntoUtf8PathBuf {
    /// Convert a [`PathBuf`] to a [`Utf8PathBuf`], panicking if the path is not valid UTF-8.
    fn into_utf8_path_buf(self) -> Utf8PathBuf;
}

impl ToUtf8Path for Path {
    fn to_utf8_path(&self) -> &Utf8Path {
        Utf8Path::from_path(self).expect("Path is not valid UTF-8")
    }
}

impl ToUtf8Path for PathBuf {
    fn to_utf8_path(&self) -> &Utf8Path {
        Utf8Path::from_path(self.as_path()).expect("Path is not valid UTF-8")
    }
}

impl ToUtf8Path for Utf8Path {
    fn to_utf8_path(&self) -> &Utf8Path {
        self
    }
}

impl ToUtf8Path for Utf8PathBuf {
    fn to_utf8_path(&self) -> &Utf8Path {
        self
    }
}

impl IntoUtf8PathBuf for PathBuf {
    fn into_utf8_path_buf(self) -> Utf8PathBuf {
        Utf8PathBuf::from_path_buf(self).expect("PathBuf is not valid UTF-8")
    }
}

pub trait Simplified {
    /// Simplify a [`Path`].
    ///
    /// On Windows, this will strip the `\\?\` prefix from paths. On other platforms, it's a no-op.
    fn simplified(&self) -> &Utf8Path;

    /// Render a [`Path`] for user-facing display.
    ///
    /// Like [`simplified`], but relativizes the path against the current working directory.
    fn user_display(&self) -> &Utf8Path;
}

impl<T: AsRef<Utf8Path>> Simplified for T {
    fn simplified(&self) -> &Utf8Path {
        Utf8Path::from_path(dunce::simplified(self.as_ref().as_std_path()))
            .expect("Path is not valid UTF-8")
    }

    fn user_display(&self) -> &Utf8Path {
        let path = self.simplified();

        // If current working directory is root, display the path as-is.
        if CWD.ancestors().nth(1).is_none() {
            return path;
        }

        // Attempt to strip the current working directory, then the canonicalized current working
        // directory, in case they differ.
        path.strip_prefix(CWD.simplified()).unwrap_or(path)
    }
}

#[cfg(test)]
mod tests {
    use super::{PathClean, clean};
    use std::path::{Path, PathBuf};

    #[test]
    fn test_empty_path_is_current_dir() {
        assert_eq!(clean(""), PathBuf::from("."));
    }

    #[test]
    fn test_clean_paths_dont_change() {
        let tests = vec![(".", "."), ("..", ".."), ("/", "/")];

        for test in tests {
            assert_eq!(clean(test.0), PathBuf::from(test.1));
        }
    }

    #[test]
    fn test_replace_multiple_slashes() {
        let tests = vec![
            ("/", "/"),
            ("//", "/"),
            ("///", "/"),
            (".//", "."),
            ("//..", "/"),
            ("..//", ".."),
            ("/..//", "/"),
            ("/.//./", "/"),
            ("././/./", "."),
            ("path//to///thing", "path/to/thing"),
            ("/path//to///thing", "/path/to/thing"),
        ];

        for test in tests {
            assert_eq!(clean(test.0), PathBuf::from(test.1));
        }
    }

    #[test]
    fn test_eliminate_current_dir() {
        let tests = vec![
            ("./", "."),
            ("/./", "/"),
            ("./test", "test"),
            ("./test/./path", "test/path"),
            ("/test/./path/", "/test/path"),
            ("test/path/.", "test/path"),
        ];

        for test in tests {
            assert_eq!(clean(test.0), PathBuf::from(test.1));
        }
    }

    #[test]
    fn test_eliminate_parent_dir() {
        let tests = vec![
            ("/..", "/"),
            ("/../test", "/test"),
            ("test/..", "."),
            ("test/path/..", "test"),
            ("test/../path", "path"),
            ("/test/../path", "/path"),
            ("test/path/../../", "."),
            ("test/path/../../..", ".."),
            ("/test/path/../../..", "/"),
            ("/test/path/../../../..", "/"),
            ("test/path/../../../..", "../.."),
            ("test/path/../../another/path", "another/path"),
            ("test/path/../../another/path/..", "another"),
            ("../test", "../test"),
            ("../test/", "../test"),
            ("../test/path", "../test/path"),
            ("../test/..", ".."),
        ];

        for test in tests {
            assert_eq!(clean(test.0), PathBuf::from(test.1));
        }
    }

    #[test]
    fn test_pathbuf_trait() {
        assert_eq!(
            PathBuf::from("/test/../path/").clean(),
            PathBuf::from("/path")
        );
    }

    #[test]
    fn test_path_trait() {
        assert_eq!(Path::new("/test/../path/").clean(), PathBuf::from("/path"));
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn test_windows_paths() {
        let tests = vec![
            ("\\..", "\\"),
            ("\\..\\test", "\\test"),
            ("test\\..", "."),
            ("test\\path\\..\\..\\..", ".."),
            ("test\\path/..\\../another\\path", "another\\path"), // Mixed
            ("test\\path\\my/path", "test\\path\\my\\path"),      // Mixed 2
            ("/dir\\../otherDir/test.json", "/otherDir/test.json"), // User example
            ("c:\\test\\..", "c:\\"),                             // issue #12
            ("c:/test/..", "c:/"),                                // issue #12
        ];

        for test in tests {
            assert_eq!(clean(test.0), PathBuf::from(test.1));
        }
    }
}
