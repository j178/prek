use std::{ffi::OsStr, fs, path::Path};

/// Windows directory entries carry cached hidden attributes; Unix uses the name.
pub(crate) fn is_hidden_entry(entry: &fs::DirEntry, path: &Path) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if let Ok(metadata) = entry.metadata() {
            if winapi_util::file::is_hidden(u64::from(metadata.file_attributes())) {
                return true;
            }
        }
    }
    #[cfg(not(windows))]
    let _ = entry;
    is_hidden_path_only(path)
}

fn is_hidden_path_only(path: &Path) -> bool {
    file_name(path).is_some_and(|name| name.as_encoded_bytes().starts_with(b"."))
}

/// Strip `prefix` from the `path` and return the remainder.
///
/// If `path` doesn't have a prefix `prefix`, then return `None`.
pub(crate) fn strip_prefix<'a, P: AsRef<Path> + ?Sized>(
    prefix: &'a P,
    path: &'a Path,
) -> Option<&'a Path> {
    #[cfg(unix)]
    fn imp<'a>(prefix: &'a Path, path: &'a Path) -> Option<&'a Path> {
        use std::os::unix::ffi::OsStrExt;

        let prefix = prefix.as_os_str().as_bytes();
        let path = path.as_os_str().as_bytes();
        if prefix.len() > path.len() || prefix != &path[0..prefix.len()] {
            None
        } else {
            Some(Path::new(OsStr::from_bytes(&path[prefix.len()..])))
        }
    }

    #[cfg(not(unix))]
    fn imp<'a>(prefix: &'a Path, path: &'a Path) -> Option<&'a Path> {
        path.strip_prefix(prefix).ok()
    }

    imp(prefix.as_ref(), path)
}

/// Returns true if this file path is just a file name. i.e., Its parent is
/// the empty string.
pub(crate) fn is_file_name<P: AsRef<Path>>(path: P) -> bool {
    #[cfg(unix)]
    {
        memchr::memchr(b'/', path.as_ref().as_os_str().as_encoded_bytes()).is_none()
    }
    #[cfg(not(unix))]
    {
        path.as_ref()
            .parent()
            .map(|p| p.as_os_str().is_empty())
            .unwrap_or(false)
    }
}

/// The final component of the path, if it is a normal file.
///
/// If the path terminates in `.`, `..`, or consists solely of a root of
/// prefix, this will return `None`.
pub(crate) fn file_name<P: AsRef<Path> + ?Sized>(path: &P) -> Option<&OsStr> {
    #[cfg(unix)]
    fn imp(path: &Path) -> Option<&OsStr> {
        use std::os::unix::ffi::OsStrExt;

        use memchr::memrchr;

        let path = path.as_os_str().as_bytes();
        if path.is_empty() {
            return None;
        }
        let last_slash = memrchr(b'/', path).map(|i| i + 1).unwrap_or(0);
        let name = &path[last_slash..];
        // Only a final component that is exactly `.` or `..` has no file
        // name. A real name that merely ends in a `.`, like `.foo.`, does.
        if name == b"." || name == b".." {
            return None;
        }
        Some(OsStr::from_bytes(name))
    }

    #[cfg(not(unix))]
    fn imp(path: &Path) -> Option<&OsStr> {
        path.file_name()
    }

    imp(path.as_ref())
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::path::Path;

    use super::{file_name, is_hidden_path_only};

    /// Extracts the last path component without requiring a filesystem entry.
    #[test]
    fn file_name_basic() {
        assert_eq!(file_name("a"), Some(OsStr::new("a")));
        assert_eq!(file_name("a/b"), Some(OsStr::new("b")));
        assert_eq!(file_name("/a/b.txt"), Some(OsStr::new("b.txt")));
    }

    // On Windows, `file_name` defers to `std`, which normalizes a trailing
    // `.` component away, so these only apply to the Unix implementation.
    #[cfg(unix)]
    /// Dot and parent components are not treated as ordinary filenames.
    #[test]
    fn file_name_dot_components() {
        assert_eq!(file_name(""), None);
        assert_eq!(file_name("."), None);
        assert_eq!(file_name(".."), None);
        assert_eq!(file_name("a/."), None);
        assert_eq!(file_name("a/.."), None);
        assert_eq!(file_name("/a/b/.."), None);
    }

    /// Preserves real filenames ending in one or more dots.
    #[test]
    fn file_name_trailing_dot() {
        assert_eq!(file_name("a."), Some(OsStr::new("a.")));
        assert_eq!(file_name("a/.b."), Some(OsStr::new(".b.")));
        assert_eq!(file_name("a/..."), Some(OsStr::new("...")));
        assert_eq!(file_name("a/..b.."), Some(OsStr::new("..b..")));
    }

    /// Trailing dots do not make a hidden filename visible.
    #[test]
    fn hidden_trailing_dot() {
        assert!(is_hidden_path_only(Path::new(".foo")));
        assert!(is_hidden_path_only(Path::new("a/.foo.")));
        assert!(is_hidden_path_only(Path::new("a/.foo..")));
        assert!(is_hidden_path_only(Path::new("a/...")));
        assert!(!is_hidden_path_only(Path::new("a/foo.")));
        assert!(!is_hidden_path_only(Path::new("a/.")));
        assert!(!is_hidden_path_only(Path::new("a/..")));
    }
}
