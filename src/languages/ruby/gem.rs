use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use prek_consts::env_vars::EnvVars;
use rustc_hash::FxHashSet;
use tracing::debug;

use crate::languages::ruby::installer::RubyResult;
use crate::process::Cmd;

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

/// Install gems to an isolated `GEM_HOME`
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

    // Use `ruby -S gem` instead of calling gem directly to work around Windows
    // issue where gem.cmd/.bat can't be executed directly (os error 193)
    let mut cmd = Cmd::new(ruby.ruby_bin(), "gem install");
    cmd.arg("-S")
        .arg("gem")
        .arg("install")
        .arg("--no-document") // Skip rdoc/ri
        .arg("--no-format-executable") // Don't rename executables to match ruby executable name
        .arg("--no-user-install") // Don't write to ~/.gem
        .arg("--install-dir")
        .arg(gem_home)
        .arg("--bindir")
        .arg(gem_home.join("bin"))
        .args(gem_files)
        .args(additional_dependencies);

    // Set environment for isolation
    cmd.env(EnvVars::GEM_HOME, gem_home)
        .env(EnvVars::BUNDLE_IGNORE_CONFIG, "1")
        .env_remove(EnvVars::GEM_PATH)
        .env_remove(EnvVars::BUNDLE_GEMFILE);

    debug!("Installing gems to {}", gem_home.display());

    cmd.check(true).output().await?;

    Ok(())
}
