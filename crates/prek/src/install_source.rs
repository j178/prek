use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
};

/// Represents how prek was installed on the system.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InstallSource {
    Homebrew,
    Cargo,
}

impl InstallSource {
    /// Detect the install source from a given path.
    fn from_path(path: &Path) -> Option<Self> {
        let canonical = path.canonicalize().unwrap_or_else(|_| PathBuf::from(path));
        let components: Vec<_> = canonical
            .components()
            .map(|c| c.as_os_str().to_owned())
            .collect();

        // Check for Homebrew Cellar installation: .../Cellar/prek/...
        let cellar = OsStr::new("Cellar");
        let prek = OsStr::new("prek");
        if components
            .windows(2)
            .any(|w| w[0] == cellar && w[1] == prek)
        {
            return Some(Self::Homebrew);
        }

        // Check for cargo bin installation: .../.cargo/bin/...
        let cargo = OsStr::new(".cargo");
        let bin = OsStr::new("bin");
        if components.windows(2).any(|w| w[0] == cargo && w[1] == bin) {
            return Some(Self::Cargo);
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
            Self::Cargo => "cargo",
        }
    }

    /// Returns the command to update prek for this install source.
    pub(crate) fn update_instructions(self) -> &'static str {
        match self {
            Self::Homebrew => "brew update && brew upgrade prek",
            Self::Cargo => "cargo install prek",
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
    fn detects_cargo_bin_macos() {
        assert_eq!(
            InstallSource::from_path(Path::new("/Users/user/.cargo/bin/prek")),
            Some(InstallSource::Cargo)
        );
    }

    #[test]
    fn detects_cargo_bin_linux() {
        assert_eq!(
            InstallSource::from_path(Path::new("/home/user/.cargo/bin/prek")),
            Some(InstallSource::Cargo)
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
    fn detects_cargo_bin_windows() {
        assert_eq!(
            InstallSource::from_path(Path::new(r"C:\Users\user\.cargo\bin\prek.exe")),
            Some(InstallSource::Cargo)
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
