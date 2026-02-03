use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};

/// Represents how prek was installed on the system.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InstallSource {
    Homebrew,
}

impl InstallSource {
    /// Detect the install source from a given path.
    fn from_path(path: &Path) -> Option<Self> {
        let canonical = path.canonicalize().unwrap_or_else(|_| PathBuf::from(path));
        let components: Vec<_> = canonical.components().map(Component::as_os_str).collect();

        // Check for Homebrew Cellar installation: .../Cellar/prek/...
        let cellar = OsStr::new("Cellar");
        let prek = OsStr::new("prek");
        if components
            .windows(2)
            .any(|w| w[0] == cellar && w[1] == prek)
        {
            return Some(Self::Homebrew);
        }

        None
    }

    /// Detect the install source from the current executable path.
    pub(crate) fn detect() -> Option<Self> {
        Self::from_path(&std::env::current_exe().ok()?)
    }

    /// Returns a human-readable description of the install source.
    pub(crate) fn description(self) -> &'static str {
        match self {
            Self::Homebrew => "Homebrew",
        }
    }

    /// Returns the command to update prek for this install source.
    pub(crate) fn update_instructions(self) -> &'static str {
        match self {
            Self::Homebrew => "brew update && brew upgrade prek",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_homebrew_cellar_arm() {
        assert_eq!(
            InstallSource::from_path(Path::new("/opt/homebrew/Cellar/prek/0.3.1/bin/prek")),
            Some(InstallSource::Homebrew)
        );
    }

    #[test]
    fn detects_homebrew_cellar_intel() {
        assert_eq!(
            InstallSource::from_path(Path::new("/usr/local/Cellar/prek/0.3.1/bin/prek")),
            Some(InstallSource::Homebrew)
        );
    }

    #[test]
    fn returns_none_for_unknown_unix_path() {
        assert_eq!(
            InstallSource::from_path(Path::new("/usr/local/bin/prek")),
            None
        );
    }

    #[test]
    fn does_not_match_other_cellar_formula() {
        assert_eq!(
            InstallSource::from_path(Path::new("/opt/homebrew/Cellar/other/0.1.0/bin/prek")),
            None
        );
    }

    #[test]
    #[cfg(windows)]
    fn returns_none_for_unknown_windows_path() {
        assert_eq!(
            InstallSource::from_path(Path::new(r"C:\Program Files\prek\prek.exe")),
            None
        );
    }
}
