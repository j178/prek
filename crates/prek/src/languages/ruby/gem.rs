use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use futures::{StreamExt, TryStreamExt};
use prek_consts::env_vars::EnvVars;
use rustc_hash::{FxHashMap, FxHashSet};
use tracing::debug;

use crate::languages::ruby::installer::RubyResult;
use crate::process::Cmd;
use crate::run::CONCURRENCY;

/// Find all .gemspec files in a directory
fn find_gemspecs(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut gemspecs = Vec::new();

    for entry in fs_err::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();

        if path.extension() == Some(OsStr::new("gemspec")) {
            gemspecs.push(path);
        }
    }

    if gemspecs.is_empty() {
        anyhow::bail!("No .gemspec files found in {}", dir.display());
    }

    Ok(gemspecs)
}

/// Build a gemspec into a .gem file
async fn build_gemspec(ruby: &RubyResult, gemspec_path: &Path) -> Result<PathBuf> {
    let repo_dir = gemspec_path
        .parent()
        .context("Gemspec has no parent directory")?;

    debug!("Building gemspec: {}", gemspec_path.display());

    // Use `ruby -S gem` instead of calling gem directly to work around Windows
    // issue where gem.cmd/.bat can't be executed directly (os error 193)
    let output = Cmd::new(ruby.ruby_bin(), "gem build")
        .arg("-S")
        .arg("gem")
        .arg("build")
        .arg(gemspec_path.file_name().unwrap())
        .current_dir(repo_dir)
        .check(true)
        .output()
        .await?;

    // Parse output to find generated .gem file
    let output_str = String::from_utf8_lossy(&output.stdout);
    let gem_file = output_str
        .lines()
        .find(|line| line.contains("File:"))
        .and_then(|line| line.split_whitespace().last())
        .context("Could not find generated .gem file in output")?;

    let gem_path = repo_dir.join(gem_file);

    if !gem_path.exists() {
        anyhow::bail!("Generated gem file not found: {}", gem_path.display());
    }

    Ok(gem_path)
}

/// Build all gemspecs in a repository, returning the list of gems built
pub(crate) async fn build_gemspecs(ruby: &RubyResult, repo_dir: &Path) -> Result<Vec<PathBuf>> {
    let gemspecs = find_gemspecs(repo_dir)?;

    let mut gem_files = Vec::new();
    for gemspec in gemspecs {
        let gem_file = build_gemspec(ruby, &gemspec).await?;
        gem_files.push(gem_file);
    }

    Ok(gem_files)
}

/// Set common gem environment variables for isolation.
fn gem_env<'a>(cmd: &'a mut Cmd, gem_home: &Path) -> &'a mut Cmd {
    cmd.env(EnvVars::GEM_HOME, gem_home)
        .env(EnvVars::BUNDLE_IGNORE_CONFIG, "1")
        .env_remove(EnvVars::GEM_PATH)
        .env_remove(EnvVars::BUNDLE_GEMFILE);

    // Parallelize native extension compilation (e.g. prism's C code).
    // Respect existing MAKEFLAGS if set (user may need to limit parallelism
    // in memory-constrained environments like Docker).
    if EnvVars::var_os("MAKEFLAGS").is_none() {
        cmd.env("MAKEFLAGS", format!("-j{}", *CONCURRENCY));
    }

    cmd
}

/// A gem resolved by `gem install --explain`.
#[derive(Debug, PartialEq)]
struct ResolvedGem {
    name: String,
    version: String,
    /// Platform suffix for pre-built binary gems (e.g. `x86_64-linux`, `java`).
    platform: Option<String>,
}

impl ResolvedGem {
    /// The `name-version[-platform]` key, matching `.gem` file stems.
    fn key(&self) -> String {
        match &self.platform {
            Some(p) => format!("{}-{}-{}", self.name, self.version, p),
            None => format!("{}-{}", self.name, self.version),
        }
    }
}

/// Parse `gem install --explain` output into resolved gems.
///
/// Splits at the rightmost `-` where the suffix starts with a digit to find
/// the version boundary, handling gem names with hyphens (e.g.
/// `ruby-progressbar-1.13.0`) and platform-specific gems (e.g.
/// `prism-1.9.0-x86_64-linux`).
fn parse_explain_output(output: &str) -> Vec<ResolvedGem> {
    output
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            // Find rightmost '-' where the suffix starts with a digit (version boundary)
            let version_start = trimmed.rmatch_indices('-').find_map(|(i, _)| {
                trimmed
                    .as_bytes()
                    .get(i + 1)
                    .filter(|b| b.is_ascii_digit())
                    .map(|_| i)
            })?;
            let name = &trimmed[..version_start];
            if name.is_empty() {
                return None;
            }
            let rest = &trimmed[version_start + 1..];

            // Split version from platform: gem versions use dots (not hyphens),
            // so the first hyphen-delimited segment starting with a non-digit
            // begins the platform suffix (e.g. "1.9.0-x86_64-linux").
            let (version, platform) = match rest.find('-') {
                Some(i)
                    if rest
                        .as_bytes()
                        .get(i + 1)
                        .is_some_and(|b| !b.is_ascii_digit()) =>
                {
                    (&rest[..i], Some(&rest[i + 1..]))
                }
                _ => (rest, None),
            };

            Some(ResolvedGem {
                name: name.to_string(),
                version: version.to_string(),
                platform: platform.map(String::from),
            })
        })
        .collect()
}

/// Resolve the full dependency list via `gem install --explain`.
async fn resolve_gems(
    ruby: &RubyResult,
    gem_home: &Path,
    gem_files: &[PathBuf],
    additional_dependencies: &FxHashSet<String>,
) -> Result<Vec<ResolvedGem>> {
    let mut cmd = Cmd::new(ruby.ruby_bin(), "gem install --explain");
    cmd.arg("-S")
        .arg("gem")
        .arg("install")
        .arg("--explain")
        .arg("--no-document")
        .arg("--no-format-executable")
        .arg("--no-user-install")
        .arg("--install-dir")
        .arg(gem_home)
        .arg("--bindir")
        .arg(gem_home.join("bin"))
        .args(gem_files)
        .args(additional_dependencies);
    gem_env(&mut cmd, gem_home);

    let output = cmd.check(true).output().await?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(parse_explain_output(&stdout))
}

/// Install a single gem with `--ignore-dependencies`.
async fn install_single_gem(
    ruby: &RubyResult,
    gem_home: &Path,
    gem: &ResolvedGem,
    local_path: Option<&Path>,
) -> Result<()> {
    let mut cmd = Cmd::new(ruby.ruby_bin(), format!("gem install {}", gem.name));
    cmd.arg("-S")
        .arg("gem")
        .arg("install")
        .arg("--ignore-dependencies")
        .arg("--no-document")
        .arg("--no-format-executable")
        .arg("--no-user-install")
        .arg("--install-dir")
        .arg(gem_home)
        .arg("--bindir")
        .arg(gem_home.join("bin"));

    if let Some(path) = local_path {
        cmd.arg(path);
    } else {
        cmd.arg(&gem.name).arg("-v").arg(&gem.version);
        // Request the specific platform variant when a pre-built binary gem was resolved
        if let Some(platform) = &gem.platform {
            cmd.arg("--platform").arg(platform);
        }
    }

    gem_env(&mut cmd, gem_home);
    cmd.check(true).output().await?;
    Ok(())
}

/// Fallback: install all gems in a single sequential `gem install` command.
async fn install_gems_sequential(
    ruby: &RubyResult,
    gem_home: &Path,
    gem_files: &[PathBuf],
    additional_dependencies: &FxHashSet<String>,
) -> Result<()> {
    let mut cmd = Cmd::new(ruby.ruby_bin(), "gem install");
    cmd.arg("-S")
        .arg("gem")
        .arg("install")
        .arg("--no-document")
        .arg("--no-format-executable")
        .arg("--no-user-install")
        .arg("--install-dir")
        .arg(gem_home)
        .arg("--bindir")
        .arg(gem_home.join("bin"))
        .args(gem_files)
        .args(additional_dependencies);
    gem_env(&mut cmd, gem_home);

    debug!("Installing gems sequentially to {}", gem_home.display());
    cmd.check(true).output().await?;
    Ok(())
}

/// Install gems to an isolated `GEM_HOME`.
///
/// Resolves the full dependency graph via `gem install --explain`, then installs
/// each gem in parallel with `--ignore-dependencies`. Falls back to a single
/// sequential `gem install` if resolution fails.
pub(crate) async fn install_gems(
    ruby: &RubyResult,
    gem_home: &Path,
    repo_path: Option<&Path>,
    additional_dependencies: &FxHashSet<String>,
) -> Result<()> {
    let mut gem_files = Vec::new();

    // Collect gems from repository. Many of these were probably built from gemspecs earlier,
    // but install all .gem files found (matches pre-commit behavior)
    if let Some(repo) = repo_path {
        for entry in fs_err::read_dir(repo)? {
            let entry = entry?;
            let path = entry.path();

            if path.extension() == Some(OsStr::new("gem")) {
                gem_files.push(path);
            }
        }
    }

    // If there are no gems and no additional dependencies, skip installation
    if gem_files.is_empty() && additional_dependencies.is_empty() {
        debug!("No gems to install, skipping gem install");
        return Ok(());
    }

    // Map "name-version" → local .gem path, so parallel installs can use local files
    let local_gem_map: FxHashMap<&str, &Path> = gem_files
        .iter()
        .filter_map(|path| {
            let stem = path.file_stem()?.to_str()?;
            Some((stem, path.as_path()))
        })
        .collect();

    match resolve_gems(ruby, gem_home, &gem_files, additional_dependencies).await {
        Ok(gems) if !gems.is_empty() => {
            debug!("Installing {} gems in parallel", gems.len());

            futures::stream::iter(gems)
                .map(|gem| {
                    let key = gem.key();
                    let local_path = local_gem_map.get(key.as_str()).copied();
                    async move { install_single_gem(ruby, gem_home, &gem, local_path).await }
                })
                .buffer_unordered(*CONCURRENCY)
                .try_collect::<Vec<()>>()
                .await?;

            Ok(())
        }
        Ok(_) => {
            debug!("gem install --explain returned no gems, falling back to sequential install");
            install_gems_sequential(ruby, gem_home, &gem_files, additional_dependencies).await
        }
        Err(err) => {
            debug!("gem install --explain failed ({err:#}), falling back to sequential install");
            install_gems_sequential(ruby, gem_home, &gem_files, additional_dependencies).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gem(name: &str, version: &str, platform: Option<&str>) -> ResolvedGem {
        ResolvedGem {
            name: name.into(),
            version: version.into(),
            platform: platform.map(Into::into),
        }
    }

    #[test]
    fn test_parse_explain_output() {
        let output = "\
Gems to install:
  unicode-emoji-4.1.0
  ruby-progressbar-1.13.0
  rubocop-ast-1.44.1
  rubocop-1.82.0
";
        let gems = parse_explain_output(output);
        assert_eq!(
            gems,
            vec![
                gem("unicode-emoji", "4.1.0", None),
                gem("ruby-progressbar", "1.13.0", None),
                gem("rubocop-ast", "1.44.1", None),
                gem("rubocop", "1.82.0", None),
            ]
        );
    }

    #[test]
    fn test_parse_explain_output_empty() {
        assert!(parse_explain_output("").is_empty());
        assert!(parse_explain_output("Gems to install:\n").is_empty());
    }

    #[test]
    fn test_parse_explain_output_platform_gems() {
        let output = "  prism-1.9.0-x86_64-linux\n  json-2.18.1-java\n";
        let gems = parse_explain_output(output);
        assert_eq!(
            gems,
            vec![
                gem("prism", "1.9.0", Some("x86_64-linux")),
                gem("json", "2.18.1", Some("java")),
            ]
        );
    }

    #[test]
    fn test_parse_explain_output_edge_cases() {
        // No version separator
        assert!(parse_explain_output("  rubocop").is_empty());
        // Empty name (leading dash)
        assert!(parse_explain_output("  -1.0.0").is_empty());
        // Pre-release version with dot separator (RubyGems convention)
        let gems = parse_explain_output("  foo-bar-0.1.0.beta");
        assert_eq!(gems, vec![gem("foo-bar", "0.1.0.beta", None)]);
    }

    #[test]
    fn test_resolved_gem_key() {
        assert_eq!(gem("rubocop", "1.82.0", None).key(), "rubocop-1.82.0");
        assert_eq!(
            gem("prism", "1.9.0", Some("x86_64-linux")).key(),
            "prism-1.9.0-x86_64-linux"
        );
    }
}
