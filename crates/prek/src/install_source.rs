use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};

/// Represents how prek was installed on the system.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InstallSource {
    Homebrew,
    Mise,
    StandaloneInstaller,
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

        // Check for mise installation: .../mise/installs/prek/...
        let mise = OsStr::new("mise");
        let installs = OsStr::new("installs");
        if components
            .windows(3)
            .any(|w| w[0] == mise && w[1] == installs && w[2] == prek)
        {
            return Some(Self::Mise);
        }

        // Check for standalone installer installation
        #[cfg(feature = "self-update")]
        match Self::is_standalone_installer() {
            Ok(true) => return Some(Self::StandaloneInstaller),
            Ok(false) => {}
            Err(e) => tracing::warn!("Failed to check for standalone installer: {}", e),
        }

        None
    }

    #[cfg(feature = "self-update")]
    fn is_standalone_installer() -> anyhow::Result<bool> {
        use axoupdater::AxoUpdater;

        let mut updater = AxoUpdater::new_for("prek");
        let updater = updater.load_receipt()?;
        Ok(updater.check_receipt_is_for_this_executable()?)
    }

    /// Detect the install source from the current executable path.
    pub(crate) fn detect() -> Option<Self> {
        Self::from_path(&std::env::current_exe().ok()?)
    }

    /// Returns a human-readable description of the install source.
    pub(crate) fn description(self) -> &'static str {
        match self {
            Self::Homebrew => "Homebrew",
            Self::Mise => "mise",
            Self::StandaloneInstaller => "the standalone installer",
        }
    }

    /// Returns the command to update prek for this install source.
    pub(crate) fn update_instructions(self) -> &'static str {
        match self {
            Self::Homebrew => "brew update && brew upgrade prek",
            Self::Mise => "mise upgrade prek",
            Self::StandaloneInstaller => "prek self update",
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
    fn detects_mise_installs() {
        assert_eq!(
            InstallSource::from_path(Path::new(
                "/Users/jo/.local/share/mise/installs/prek/0.3.1/bin/prek"
            )),
            Some(InstallSource::Mise)
        );
    }

    #[test]
    fn does_not_match_other_mise_tool() {
        assert_eq!(
            InstallSource::from_path(Path::new(
                "/Users/jo/.local/share/mise/installs/ruby/3.4.6/bin/ruby"
            )),
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
