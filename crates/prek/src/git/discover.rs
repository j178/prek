//! Fast discovery for standard, locally owned work trees. Anything else uses Git.
//!
//! This only checks repository discovery settings, not Git's validation of
//! unrelated settings in local, global, or system config. Other platforms keep
//! using Git until their ownership and filesystem boundaries can be checked.

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use prek_consts::env_vars::{EnvVars, EnvVarsRead};

pub(super) fn root(preserved_work_tree: Option<&Path>) -> Result<PathBuf> {
    for name in [
        "GIT_WORK_TREE",
        "GIT_COMMON_DIR",
        "GIT_OBJECT_DIRECTORY",
        "GIT_IMPLICIT_WORK_TREE",
        "GIT_DISCOVERY_ACROSS_FILESYSTEM",
        "GIT_CEILING_DIRECTORIES",
        "GIT_CONFIG_COUNT",
        "GIT_CONFIG_PARAMETERS",
        "GIT_TEST_ASSUME_DIFFERENT_OWNER",
    ] {
        ensure!(!EnvVars.is_set(name), "{name} requires Git discovery");
    }

    let cwd = std::env::current_dir()?;
    let device = fs_err::metadata(&cwd)?.dev();
    let mut work_tree = cwd.as_path();
    loop {
        ensure!(
            fs_err::metadata(work_tree)?.dev() == device,
            "Filesystem boundary"
        );
        // A bare repository or a search inside .git must not find an outer work tree.
        ensure!(
            !work_tree.join("HEAD").try_exists()?,
            "Possible Git directory"
        );
        match fs_err::symlink_metadata(work_tree.join(".git")) {
            Ok(_) => break,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err.into()),
        }
        work_tree = work_tree.parent().context("No repository found")?;
    }
    owned_metadata(work_tree)?;
    let marker = work_tree.join(".git");
    let git_dir = if owned_metadata(&marker)?.is_dir() {
        marker
    } else {
        dunce::canonicalize(read_path(&marker, "gitdir: ")?)?
    };
    ensure!(owned_metadata(&git_dir)?.is_dir(), "Invalid Git directory");
    if let Some(git_dir_env) = EnvVars.var_os(EnvVars::GIT_DIR) {
        ensure!(
            !git_dir_env.is_empty() && same_file::is_same_file(cwd.join(git_dir_env), &git_dir)?,
            "GIT_DIR overrides discovery"
        );
        let preserved = preserved_work_tree.context("GIT_DIR requires a preserved work tree")?;
        ensure!(
            same_file::is_same_file(preserved, work_tree)?,
            "Different work tree"
        );
    }

    let common_file = git_dir.join("commondir");
    let common_dir = if common_file.try_exists()? {
        dunce::canonicalize(read_path(&common_file, "")?)?
    } else {
        git_dir.clone()
    };
    owned_metadata(&common_dir)?;
    for name in ["objects", "refs"] {
        let directory = common_dir.join(name);
        ensure!(directory.is_dir(), "Incomplete repository");
        rustix::fs::access(&directory, rustix::fs::Access::EXEC_OK)?;
    }
    let head = read(&git_dir.join("HEAD"))?;
    let head = head.trim_ascii();
    ensure!(
        (head.starts_with("ref: refs/") && head.len() > "ref: refs/".len())
            || (head.len() == 40 && head.bytes().all(|byte| byte.is_ascii_hexdigit())),
        "Unsupported HEAD"
    );
    check_config(&read(&common_dir.join("config"))?, &common_dir, work_tree)?;
    Ok(work_tree.to_path_buf())
}

fn owned_metadata(path: &Path) -> Result<std::fs::Metadata> {
    let metadata = fs_err::symlink_metadata(path)?;
    // Leave symlinked metadata, safe.directory, and sudo exceptions to Git.
    ensure!(
        !metadata.file_type().is_symlink() && metadata.uid() == rustix::process::geteuid().as_raw(),
        "Ownership requires Git discovery"
    );
    Ok(metadata)
}

fn read(path: &Path) -> Result<String> {
    let metadata = fs_err::symlink_metadata(path)?;
    ensure!(
        metadata.is_file() && metadata.len() <= 65536,
        "Nonstandard metadata file"
    );
    let contents = fs_err::read_to_string(path)?;
    ensure!(!contents.contains('\0'), "Invalid metadata");
    Ok(contents)
}

fn read_path(file: &Path, prefix: &str) -> Result<PathBuf> {
    let contents = read(file)?;
    let path = contents.strip_prefix(prefix).context("Invalid Git file")?;
    let path = path.trim_end_matches(['\r', '\n']);
    ensure!(
        !path.is_empty() && !path.contains(['\r', '\n']),
        "Invalid Git path"
    );
    Ok(file.parent().context("Missing parent")?.join(path))
}

/// Accept only simple config that agrees with the discovered layout. This does
/// not interpret quotes, escapes, includes, extensions, or last-value precedence.
fn check_config(contents: &str, git_dir: &Path, work_tree: &Path) -> Result<()> {
    ensure!(!contents.contains('\\'), "Escaped config requires Git");
    let mut section_seen = false;
    for line in contents.lines().map(str::trim_ascii) {
        if line.is_empty() || line.starts_with(['#', ';']) {
            continue;
        }
        if let Some(header) = line.strip_prefix('[') {
            let header = header.strip_suffix(']').context("Complex config section")?;
            let (name, subsection) = header.split_once(' ').unwrap_or((header, ""));
            ensure!(
                !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'),
                "Complex config section"
            );
            if !subsection.is_empty() {
                let subsection = subsection
                    .trim_ascii()
                    .strip_prefix('"')
                    .and_then(|s| s.strip_suffix('"'))
                    .context("Complex subsection")?;
                ensure!(!subsection.contains('"'), "Complex subsection");
            }
            ensure!(
                !["include", "includeif", "extensions"]
                    .iter()
                    .any(|s| name.eq_ignore_ascii_case(s)),
                "Extended config requires Git"
            );
            section_seen = true;
            continue;
        }
        ensure!(section_seen && !line.contains('"'), "Complex config value");
        let (key, value) = line.split_once('=').context("Implicit config value")?;
        let key = key.trim_ascii_end();
        ensure!(
            key.as_bytes().first().is_some_and(u8::is_ascii_alphabetic)
                && key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'),
            "Complex config key"
        );
        let value = value
            .split(['#', ';'])
            .next()
            .unwrap_or_default()
            .trim_ascii();
        // Checking every section can only cause extra fallbacks. Every occurrence
        // must agree, so section lookup and duplicate-key precedence are unnecessary.
        if key.eq_ignore_ascii_case("bare") {
            ensure!(value == "false", "Nonstandard core.bare");
        } else if key.eq_ignore_ascii_case("repositoryformatversion") {
            ensure!(value == "0", "Nonstandard repository format");
        } else if key.eq_ignore_ascii_case("worktree") {
            ensure!(
                !value.is_empty() && same_file::is_same_file(git_dir.join(value), work_tree)?,
                "Nonstandard core.worktree"
            );
        }
    }
    Ok(())
}
