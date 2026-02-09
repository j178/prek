use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};

/// Represents how prek was installed on the system.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InstallSource {
    Homebrew,
    Mise,
    UvTool,
    Pipx,
    Asdf,
    StandaloneInstaller,
}

impl InstallSource {
    /// Detect the install source from a given path.
    fn from_path(path: &Path) -> Option<Self> {
        // Resolve symlinks so e.g. ~/.local/bin/prek -> .../uv/tools/prek/bin/prek is detected.
        let canonical = path.canonicalize().unwrap_or_else(|_| PathBuf::from(path));
        let components: Vec<_> = canonical.components().map(Component::as_os_str).collect();

        /// Check whether `components` contains a contiguous subsequence matching `pattern`.
        fn contains_sequence(components: &[&OsStr], pattern: &[&OsStr]) -> bool {
            components.windows(pattern.len()).any(|w| w == pattern)
        }

        let prek = OsStr::new("prek");

        // Homebrew: .../Cellar/prek/...
        if contains_sequence(&components, &[OsStr::new("Cellar"), prek]) {
            return Some(Self::Homebrew);
        }
        // uv tool: .../uv/tools/prek/...
        if contains_sequence(&components, &[OsStr::new("uv"), OsStr::new("tools"), prek]) {
            return Some(Self::UvTool);
        }
        // pipx: .../pipx/venvs/prek/...
        if contains_sequence(
            &components,
            &[OsStr::new("pipx"), OsStr::new("venvs"), prek],
        ) {
            return Some(Self::Pipx);
        }
        // asdf: .../.asdf/installs/prek/...
        if contains_sequence(
            &components,
            &[OsStr::new(".asdf"), OsStr::new("installs"), prek],
        ) {
            return Some(Self::Asdf);
        }
        // mise: .../mise/installs/prek/...
        if contains_sequence(
            &components,
            &[OsStr::new("mise"), OsStr::new("installs"), prek],
        ) {
            return Some(Self::Mise);
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
        #[cfg(feature = "self-update")]
        match Self::is_standalone_installer() {
            Ok(true) => return Some(Self::StandaloneInstaller),
            Ok(false) => {}
            Err(e) => tracing::warn!("Failed to check for standalone installer: {e}"),
        }

        Self::from_path(&std::env::current_exe().ok()?)
    }

    /// Returns a human-readable description of the install source.
    pub(crate) fn description(self) -> &'static str {
        match self {
            Self::Homebrew => "Homebrew",
            Self::Mise => "mise",
            Self::UvTool => "uv tool",
            Self::Pipx => "pipx",
            Self::Asdf => "asdf",
            Self::StandaloneInstaller => "the standalone installer",
        }
    }

    /// Returns the command to update prek for this install source.
    pub(crate) fn update_instructions(self) -> &'static str {
        match self {
            Self::Homebrew => "brew update && brew upgrade prek",
            Self::Mise => "mise upgrade prek",
            Self::UvTool => "uv tool upgrade prek",
            Self::Pipx => "pipx upgrade prek",
            Self::Asdf => "asdf install prek latest",
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
    fn detects_uv_tool_macos() {
        assert_eq!(
            InstallSource::from_path(Path::new("/Users/user/.local/share/uv/tools/prek/bin/prek")),
            Some(InstallSource::UvTool)
        );
    }

    #[test]
    fn detects_uv_tool_linux() {
        assert_eq!(
            InstallSource::from_path(Path::new("/home/user/.local/share/uv/tools/prek/bin/prek")),
            Some(InstallSource::UvTool)
        );
    }

    #[test]
    fn detects_uv_tool_custom_xdg() {
        assert_eq!(
            InstallSource::from_path(Path::new("/opt/data/uv/tools/prek/bin/prek")),
            Some(InstallSource::UvTool)
        );
    }

    #[test]
    fn does_not_match_other_uv_tool() {
        assert_eq!(
            InstallSource::from_path(Path::new("/home/user/.local/share/uv/tools/ruff/bin/ruff")),
            None
        );
    }

    #[test]
    fn detects_pipx_macos() {
        assert_eq!(
            InstallSource::from_path(Path::new("/Users/user/.local/pipx/venvs/prek/bin/prek")),
            Some(InstallSource::Pipx)
        );
    }

    #[test]
    fn detects_pipx_linux() {
        assert_eq!(
            InstallSource::from_path(Path::new(
                "/home/user/.local/share/pipx/venvs/prek/bin/prek"
            )),
            Some(InstallSource::Pipx)
        );
    }

    #[test]
    fn does_not_match_other_pipx_package() {
        assert_eq!(
            InstallSource::from_path(Path::new("/home/user/.local/pipx/venvs/black/bin/black")),
            None
        );
    }

    #[test]
    fn detects_asdf() {
        assert_eq!(
            InstallSource::from_path(Path::new("/home/user/.asdf/installs/prek/0.3.1/bin/prek")),
            Some(InstallSource::Asdf)
        );
    }

    #[test]
    fn does_not_match_other_asdf_plugin() {
        assert_eq!(
            InstallSource::from_path(Path::new(
                "/home/user/.asdf/installs/python/3.12.0/bin/python"
            )),
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
