use std::path::Path;

use anyhow::{Context, Result};
use target_lexicon::{Architecture, HOST, OperatingSystem};
use tracing::debug;

use crate::process::Cmd;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Libc {
    Glibc,
    Musl,
}

impl Libc {
    /// Detect a libc available at the standard Linux loader paths, preferring glibc.
    ///
    /// This checks the runtime environment, independently of the libc used to build prek.
    /// It does not determine the system's preferred libc or check version compatibility.
    /// Non-Linux platforms return `None`; unsupported architectures and unrecognized
    /// Linux environments return an error, including installations at nonstandard paths.
    pub(crate) async fn detect() -> Result<Option<Self>> {
        if HOST.operating_system != OperatingSystem::Linux {
            return Ok(None);
        }

        let (glibc, musl) = match HOST.architecture {
            Architecture::X86_64 => ("/lib64/ld-linux-x86-64.so.2", "/lib/ld-musl-x86_64.so.1"),
            Architecture::Aarch64(target_lexicon::Aarch64Architecture::Aarch64) => {
                ("/lib/ld-linux-aarch64.so.1", "/lib/ld-musl-aarch64.so.1")
            }
            Architecture::Powerpc64le => ("/lib64/ld64.so.2", "/lib/ld-musl-powerpc64le.so.1"),
            arch => anyhow::bail!("Unsupported architecture for libc detection: {arch}"),
        };
        Self::detect_linux(Path::new(glibc), Path::new(musl))
            .await
            .map(Some)
    }

    async fn detect_linux(glibc: &Path, musl: &Path) -> Result<Self> {
        if is_glibc(glibc).await? {
            debug!(loader = %glibc.display(), "Detected glibc");
            return Ok(Self::Glibc);
        }

        if musl
            .try_exists()
            .with_context(|| format!("Failed to check musl loader at {}", musl.display()))?
        {
            // musl prints its identity to stderr and exits unsuccessfully without a program.
            let output = Cmd::new(musl)
                .env("LC_ALL", "C")
                .check(false)
                .output()
                .await?;
            if String::from_utf8_lossy(&output.stderr)
                .trim_start()
                .starts_with("musl libc (")
            {
                debug!(loader = %musl.display(), "Detected musl");
                return Ok(Self::Musl);
            }
        }

        anyhow::bail!(
            "Could not detect glibc or musl at {} or {}",
            glibc.display(),
            musl.display()
        )
    }
}

async fn is_glibc(loader: &Path) -> Result<bool> {
    if !loader
        .try_exists()
        .with_context(|| format!("Failed to check glibc loader at {}", loader.display()))?
    {
        return Ok(false);
    }

    // Before glibc 2.33, the loader has no --version option. These installations
    // normally link the ABI loader path to ld-<major>.<minor>.so.
    let resolved = fs_err::canonicalize(loader)?;
    if let Some(name) = resolved.file_name().and_then(|name| name.to_str())
        && is_versioned_glibc_loader(name)
    {
        return Ok(true);
    }

    // gcompat can occupy the same path. Identify glibc's banner without depending
    // on the distro-controlled package name inside the parentheses.
    let output = Cmd::new(loader)
        .arg("--version")
        .env("LC_ALL", "C")
        .check(false)
        .output()
        .await?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    Ok(output.status.success()
        && stdout
            .lines()
            .chain(stderr.lines())
            .any(|line| line.starts_with("ld.so (") && line.contains(") ")))
}

fn is_versioned_glibc_loader(name: &str) -> bool {
    let Some(version) = name
        .strip_prefix("ld-")
        .and_then(|name| name.strip_suffix(".so"))
    else {
        return false;
    };
    let Some((major, minor)) = version.split_once('.') else {
        return false;
    };

    !major.is_empty()
        && !minor.is_empty()
        && major.bytes().all(|byte| byte.is_ascii_digit())
        && minor.bytes().all(|byte| byte.is_ascii_digit())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn loader_script(path: &Path, body: &str) -> Result<()> {
        fs_err::write(path, format!("#!/bin/sh\n{body}\n"))?;
        crate::fs::make_executable(path)?;
        Ok(())
    }

    /// Old glibc must be recognized even though its loader rejects --version.
    #[tokio::test]
    async fn old_glibc_loader() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let glibc = temp.path().join("ld-linux.so");
        let legacy = temp.path().join("ld-2.31.so");
        loader_script(
            &legacy,
            "echo '--version: error while loading shared libraries' >&2\nexit 127",
        )?;
        std::os::unix::fs::symlink("ld-2.31.so", &glibc)?;

        assert_eq!(
            Libc::detect_linux(&glibc, &temp.path().join("ld-musl.so")).await?,
            Libc::Glibc
        );
        Ok(())
    }

    /// A distro-branded glibc takes precedence when both libc implementations exist.
    #[tokio::test]
    async fn glibc_preferred_over_musl() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let glibc = temp.path().join("ld-linux.so");
        let musl = temp.path().join("ld-musl.so");
        loader_script(
            &glibc,
            "test \"$1\" = --version || exit 1\necho 'ld.so (Gentoo 2.39-r6 p4) stable release version 2.39.'",
        )?;
        loader_script(&musl, "echo 'musl libc (x86_64)' >&2\nexit 1")?;

        assert_eq!(Libc::detect_linux(&glibc, &musl).await?, Libc::Glibc);
        Ok(())
    }

    /// A gcompat stub must not hide musl, whose identity probe exits unsuccessfully.
    #[tokio::test]
    async fn musl_with_or_without_gcompat() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let glibc = temp.path().join("ld-linux.so");
        let musl = temp.path().join("ld-musl.so");
        loader_script(
            &glibc,
            "echo 'This is the gcompat ELF interpreter stub.' >&2\nexit 1",
        )?;
        loader_script(
            &musl,
            "test \"$#\" = 0 || exit 1\nprintf 'musl libc (x86_64)\\nVersion 1.2.5\\nDynamic Program Loader\\n' >&2\nexit 1",
        )?;

        assert_eq!(Libc::detect_linux(&glibc, &musl).await?, Libc::Musl);
        fs_err::remove_file(&glibc)?;
        assert_eq!(Libc::detect_linux(&glibc, &musl).await?, Libc::Musl);
        Ok(())
    }

    /// Missing loaders and successful commands with unrelated output must stay unknown.
    #[tokio::test]
    async fn unrecognized_libc() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let glibc = temp.path().join("ld-linux.so");
        let musl = temp.path().join("ld-musl.so");
        assert!(Libc::detect_linux(&glibc, &musl).await.is_err());

        loader_script(&glibc, "echo 'another loader 1.0'")?;
        loader_script(&musl, "echo 'another loader 1.0' >&2")?;
        assert!(Libc::detect_linux(&glibc, &musl).await.is_err());
        Ok(())
    }

    /// Non-Linux hosts have no Linux libc to detect.
    #[cfg(not(target_os = "linux"))]
    #[tokio::test]
    async fn non_linux() -> Result<()> {
        assert_eq!(Libc::detect().await?, None);
        Ok(())
    }
}
