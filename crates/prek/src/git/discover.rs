//! Conservative repository discovery. Unsupported layouts and configuration use Git.

use std::io::Read;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use prek_consts::env_vars::{EnvVars, EnvVarsRead};

use super::path_from_git_bytes;

pub(super) fn root(preserved_work_tree: Option<&Path>) -> Result<PathBuf> {
    for name in [
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
    let explicit_git_dir = EnvVars.var_os(EnvVars::GIT_DIR);
    let (marker, inferred_work_tree) = if let Some(git_dir) = &explicit_git_dir {
        ensure!(!git_dir.is_empty(), "GIT_DIR is empty");
        (cwd.join(git_dir), cwd.clone())
    } else {
        let work_tree = find_work_tree(&cwd)?;
        (work_tree.join(".git"), work_tree)
    };
    check_owner(&marker)?;
    let git_dir = if fs_err::metadata(&marker)?.is_dir() {
        dunce::canonicalize(&marker)?
    } else {
        let path = read_path(&marker, b"gitdir: ")?;
        let parent = marker.parent().context("Git file has no parent")?;
        dunce::canonicalize(parent.join(path))?
    };
    check_owner(&git_dir)?;
    let common_dir_file = git_dir.join("commondir");
    let common_dir = if common_dir_file.try_exists()? {
        dunce::canonicalize(git_dir.join(read_path(&common_dir_file, b"")?))?
    } else {
        git_dir.clone()
    };
    check_owner(&common_dir)?;
    ensure!(
        common_dir.join("objects").is_dir() && common_dir.join("refs").is_dir(),
        "Incomplete repository"
    );
    let head = read(&git_dir.join("HEAD"))?;
    let head = head.trim_ascii();
    ensure!(
        (head.starts_with(b"ref: refs/") && head.len() > b"ref: refs/".len())
            || (head.len() == 40 && head.iter().all(u8::is_ascii_hexdigit)),
        "Unsupported HEAD"
    );
    let config = read(&common_dir.join("config"))?;
    let config = parse_config(str::from_utf8(&config)?)?;
    let root = if let Some(work_tree) = preserved_work_tree {
        work_tree.to_path_buf()
    } else if let Some(work_tree) = EnvVars.var_os(EnvVars::GIT_WORK_TREE) {
        ensure!(!work_tree.is_empty(), "GIT_WORK_TREE is empty");
        cwd.join(work_tree)
    } else if explicit_git_dir.is_some() {
        cwd
    } else if common_dir != git_dir {
        // A linked worktree does not inherit core.bare or core.worktree from the
        // main repository. parse_config leaves extensions.worktreeConfig to Git.
        inferred_work_tree
    } else {
        ensure!(!config.bare, "The repository is bare");
        match config.work_tree {
            Some(work_tree) => git_dir.join(work_tree),
            None => inferred_work_tree,
        }
    };
    check_owner(&root)?;
    ensure!(root.is_dir(), "The work tree is not a directory");
    let root = dunce::canonicalize(root)?;
    check_owner(&root)?;
    Ok(root)
}

fn find_work_tree(cwd: &Path) -> Result<PathBuf> {
    let mut candidate = cwd;
    let device = fs_err::metadata(cwd)?.dev();
    loop {
        // Git stops at bare repositories too. Let it handle those and searches
        // starting inside a git directory, rather than finding an outer worktree.
        ensure!(
            !candidate.join("HEAD").try_exists()?,
            "Possible bare repository"
        );
        match fs_err::symlink_metadata(candidate.join(".git")) {
            Ok(_) => return Ok(candidate.to_path_buf()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err.into()),
        }
        candidate = candidate.parent().context("No repository found")?;
        ensure!(
            fs_err::metadata(candidate)?.dev() == device,
            "Filesystem boundary"
        );
    }
}

fn check_owner(path: &Path) -> Result<()> {
    // Leave safe.directory and sudo ownership exceptions to Git.
    ensure!(
        fs_err::symlink_metadata(path)?.uid() == rustix::process::geteuid().as_raw(),
        "Git must check repository ownership"
    );
    Ok(())
}

fn read(path: &Path) -> Result<Vec<u8>> {
    const LIMIT: usize = 1024 * 1024;
    let mut bytes = Vec::new();
    fs_err::File::open(path)?
        .take((LIMIT + 1) as u64)
        .read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= LIMIT, "Repository metadata is too large");
    Ok(bytes)
}

fn read_path(path: &Path, prefix: &[u8]) -> Result<PathBuf> {
    let bytes = read(path)?;
    let mut value = bytes.strip_prefix(prefix).context("Invalid Git path file")?;
    while value.last().is_some_and(|b| matches!(b, b'\r' | b'\n')) {
        value = &value[..value.len() - 1];
    }
    ensure!(!value.is_empty() && !value.contains(&0), "Invalid Git path");
    path_from_git_bytes(value).map_err(Into::into)
}

#[derive(Default)]
struct Config {
    bare: bool,
    work_tree: Option<PathBuf>,
}

fn parse_config(contents: &str) -> Result<Config> {
    // Recognize a deliberately small, unambiguous subset of Git config. In
    // particular, escapes, continuations, includes, and extensions use Git.
    ensure!(!contents.contains(['\\', '\0']), "Complex Git config");
    let mut config = Config::default();
    let mut in_core = false;
    let mut section_seen = false;
    for line in contents.lines().map(str::trim_ascii) {
        if line.is_empty() || line.starts_with(['#', ';']) {
            continue;
        }
        if let Some(header) = line.strip_prefix('[') {
            let (header, tail) = header.rsplit_once(']').context("Invalid section")?;
            let tail = tail.trim_ascii();
            ensure!(
                tail.is_empty() || tail.starts_with(['#', ';']),
                "Invalid section suffix"
            );
            let (name, subsection) = match header.split_once('"') {
                Some((name, subsection)) => {
                    ensure!(
                        name.ends_with(|c: char| c.is_ascii_whitespace()),
                        "Invalid subsection separator"
                    );
                    let subsection = subsection
                        .strip_suffix('"')
                        .context("Invalid subsection")?;
                    ensure!(!subsection.contains('"'), "Complex subsection");
                    (name.trim_ascii_end(), true)
                }
                None => (header, false),
            };
            ensure!(
                !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'),
                "Complex section name"
            );
            ensure!(
                !["extensions", "include", "includeif"]
                    .iter()
                    .any(|section| name.eq_ignore_ascii_case(section)),
                "Extended Git config"
            );
            in_core = !subsection && name.eq_ignore_ascii_case("core");
            section_seen = true;
            continue;
        }
        ensure!(section_seen, "Config value outside a section");
        let end = line
            .find(|c: char| !c.is_ascii_alphanumeric() && c != '-')
            .unwrap_or(line.len());
        let key = &line[..end];
        ensure!(
            key.as_bytes().first().is_some_and(u8::is_ascii_alphabetic),
            "Invalid config key"
        );
        let rest = line[end..].trim_ascii_start();
        let value = if let Some(value) = rest.strip_prefix('=') {
            parse_value(value)?
        } else {
            ensure!(
                rest.is_empty() || rest.starts_with(['#', ';']),
                "Invalid config value"
            );
            "true".to_owned()
        };
        if !in_core {
            continue;
        }
        if key.eq_ignore_ascii_case("repositoryformatversion") {
            ensure!(value == "0", "Extended repository format");
        } else if key.eq_ignore_ascii_case("bare") {
            config.bare = match value.to_ascii_lowercase().as_str() {
                "true" | "yes" | "on" | "1" => true,
                "false" | "no" | "off" | "0" | "" => false,
                _ => bail!("Unsupported core.bare"),
            };
        } else if key.eq_ignore_ascii_case("worktree") {
            ensure!(rest.starts_with('='), "Implicit core.worktree");
            ensure!(!value.is_empty(), "Empty core.worktree");
            config.work_tree = Some(value.into());
        }
    }
    Ok(config)
}

fn parse_value(value: &str) -> Result<String> {
    let mut result = String::new();
    let mut quoted = false;
    let mut whitespace = String::new();
    for c in value.trim_ascii_start().chars() {
        match c {
            '"' => {
                result.push_str(&whitespace);
                whitespace.clear();
                quoted = !quoted;
            }
            '#' | ';' if !quoted => break,
            c if c.is_ascii_whitespace() && !quoted => whitespace.push(c),
            c => {
                result.push_str(&whitespace);
                whitespace.clear();
                result.push(c);
            }
        }
    }
    ensure!(!quoted, "Unterminated config value");
    Ok(result)
}
