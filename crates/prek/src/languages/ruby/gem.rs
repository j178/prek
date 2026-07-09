use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use prek_consts::env_vars::{EnvVars, EnvVarsRead};
use prek_consts::prepend_paths;
use rustc_hash::FxHashSet;
use tracing::debug;

use crate::languages::ruby::installer::RubyResult;
use crate::process::Cmd;
use crate::run::INTERNAL_CONCURRENCY;

/// Build a `PATH` value with the resolved Ruby's bin directory prepended.
///
/// `ruby -S gem` searches `$PATH` for the `gem` script. The rv-ruby tarballs
/// that prek auto-downloads ship `gem` next to `ruby` in the same `bin/`
/// directory, but that directory is not on the parent process's PATH (e.g.
/// in a Docker image with no system Ruby). Without prepending it, Ruby errors
/// with `LoadError: No such file or directory -- gem`.
fn ruby_path_env(ruby: &RubyResult) -> Result<OsString> {
    let ruby_bin_dir = ruby
        .ruby_bin()
        .parent()
        .context("Ruby executable should have a parent directory")?;
    prepend_paths(&[ruby_bin_dir]).context("Failed to join PATH")
}

/// Find files with the given extension directly under a directory.
fn find_top_level_files(dir: &Path, extension: &str) -> Result<Vec<PathBuf>> {
    let extension = OsStr::new(extension);
    let mut paths = Vec::new();

    for entry in fs_err::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();

        if path.extension() == Some(extension) {
            paths.push(path);
        }
    }

    Ok(paths)
}

/// Build a gemspec into a .gem file
async fn build_gemspec(ruby: &RubyResult, gemspec_path: &Path) -> Result<()> {
    let repo_dir = gemspec_path
        .parent()
        .context("Gemspec has no parent directory")?;
    let gemspec_file = gemspec_path
        .file_name()
        .context("Gemspec path has no file name")?;

    debug!("Building gemspec: {}", gemspec_path.display());

    // Use `ruby -S gem` instead of calling gem directly to work around Windows
    // issue where gem.cmd/.bat can't be executed directly (os error 193)
    Cmd::new(ruby.ruby_bin())
        .arg("-S")
        .arg("gem")
        .arg("build")
        .arg(gemspec_file)
        .current_dir(repo_dir)
        .env(EnvVars::PATH, ruby_path_env(ruby)?)
        .check(true)
        .output()
        .await?;

    Ok(())
}

/// Build all gemspecs in a repository, returning the number built
pub(crate) async fn build_gemspecs(ruby: &RubyResult, repo_dir: &Path) -> Result<usize> {
    let gemspecs = find_top_level_files(repo_dir, "gemspec")?;
    if gemspecs.is_empty() {
        anyhow::bail!("No .gemspec files found in {}", repo_dir.display());
    }

    let count = gemspecs.len();

    for gemspec in gemspecs {
        build_gemspec(ruby, &gemspec).await?;
    }

    Ok(count)
}

/// Set common gem environment variables for isolation.
///
/// Also prepends the resolved Ruby's bin directory to `$PATH` so that
/// `ruby -S gem` can find the bundled `gem` script even when no system Ruby
/// is on PATH.
fn gem_env<'a>(cmd: &'a mut Cmd, ruby: &RubyResult, gem_home: &Path) -> Result<&'a mut Cmd> {
    cmd.env(EnvVars::PATH, ruby_path_env(ruby)?)
        .env(EnvVars::GEM_HOME, gem_home)
        .env(EnvVars::BUNDLE_IGNORE_CONFIG, "1")
        .env_remove(EnvVars::GEM_PATH)
        .env_remove(EnvVars::BUNDLE_GEMFILE);

    // Parallelize native extension compilation (e.g. prism's C code).
    // Respect existing MAKEFLAGS if set (user may need to limit parallelism
    // in memory-constrained environments like Docker).
    if EnvVars.var_os("MAKEFLAGS").is_none() {
        cmd.env("MAKEFLAGS", format!("-j{}", *INTERNAL_CONCURRENCY));
    }

    Ok(cmd)
}

/// Install gems to an isolated `GEM_HOME`.
pub(crate) async fn install_gems(
    ruby: &RubyResult,
    gem_home: &Path,
    repo_path: Option<&Path>,
    additional_dependencies: &FxHashSet<String>,
) -> Result<()> {
    // Collect gems from repository. Many of these were probably built from gemspecs earlier,
    // but install all .gem files found (matches pre-commit behavior)
    let gem_files = if let Some(repo_path) = repo_path {
        find_top_level_files(repo_path, "gem")?
    } else {
        Vec::new()
    };

    // If there are no gems and no additional dependencies, skip installation
    if gem_files.is_empty() && additional_dependencies.is_empty() {
        debug!("No gems to install, skipping gem install");
        return Ok(());
    }

    let mut cmd = Cmd::new(ruby.ruby_bin());
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
    gem_env(&mut cmd, ruby, gem_home)?;

    debug!("Installing gems to {}", gem_home.display());
    cmd.check(true).output().await?;
    Ok(())
}
