use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::Result;
use assert_cmd::assert::OutputAssertExt;
use prek_consts::env_vars::{EnvVars, EnvVarsRead};

use super::{discover, git_work_tree, init_git_work_tree, path_from_git_bytes, root};

const PROBE: &str = "PREK_TEST_GIT_ROOT_PROBE";
const EXPECTED: &str = "PREK_TEST_GIT_ROOT_EXPECTED";
const GIT_CALLED: &str = "PREK_TEST_GIT_ROOT_CALLED";
const CHDIR: &str = "PREK_TEST_GIT_ROOT_CHDIR";

// Each probe gets its own cwd, environment, and OnceLocks without mutating the
// test runner's process state. Normal test discovery leaves this probe idle.
#[test]
fn root_probe() -> Result<()> {
    let Some(mode) = EnvVars.var_os(PROBE) else {
        return Ok(());
    };
    init_git_work_tree()?;
    if let Some(cwd) = EnvVars.var_os(CHDIR) {
        std::env::set_current_dir(cwd)?;
    }
    if mode == "error" {
        assert!(root().is_err());
    } else {
        if mode == "fallback" {
            assert!(discover::root(git_work_tree()).is_err());
        }
        let expected = EnvVars
            .var_os(EXPECTED)
            .ok_or_else(|| anyhow::anyhow!("Missing expected root"))?;
        assert_eq!(root()?, PathBuf::from(expected));
    }
    Ok(())
}

struct Fixture {
    dir: tempfile::TempDir,
    git: PathBuf,
}

impl Fixture {
    fn new() -> Result<Self> {
        Ok(Self {
            dir: tempfile::tempdir()?,
            git: which::which("git")?,
        })
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    fn command(executable: impl AsRef<OsStr>, cwd: &Path) -> Command {
        let mut command = Command::new(executable);
        command.current_dir(cwd);
        for (name, _) in std::env::vars_os() {
            if name.as_encoded_bytes().starts_with(b"GIT_") {
                command.env_remove(name);
            }
        }
        command
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_GLOBAL", "/dev/null");
        command
    }

    fn git(&self, cwd: &Path) -> Command {
        Self::command(&self.git, cwd)
    }

    fn init(&self, name: &str) -> Result<PathBuf> {
        let path = self.path(name);
        fs_err::create_dir_all(&path)?;
        self.git(&path).args(["init", "-q"]).assert().success();
        Ok(path)
    }

    fn commit(&self, repo: &Path) {
        self.git(repo)
            .args([
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "commit",
                "-qm",
                "initial",
                "--allow-empty",
            ])
            .assert()
            .success();
    }

    fn compare(&self, cwd: &Path, env: &[(&str, OsString)], mode: &str) -> Result<()> {
        let mut git = self.git(cwd);
        git.args(["rev-parse", "--show-toplevel"])
            .envs(env.iter().cloned());
        if let Some((_, cwd)) = env.iter().find(|(key, _)| *key == CHDIR) {
            git.current_dir(cwd);
        }
        // Match the GIT_WORK_TREE synthesized by prek at startup.
        if env.iter().any(|(key, _)| *key == "GIT_DIR")
            && !env.iter().any(|(key, _)| *key == "GIT_WORK_TREE")
        {
            git.env("GIT_WORK_TREE", cwd);
        }
        let output = git.output()?;
        let mut probe = Self::command(std::env::current_exe()?, cwd);
        probe
            .args(["--exact", "git::root_tests::root_probe", "--nocapture"])
            .envs(env.iter().cloned())
            .env(PROBE, mode);
        if mode == "error" {
            assert!(!output.status.success());
        } else {
            assert!(output.status.success(), "{output:?}");
            let bytes = output.stdout.strip_suffix(b"\n").unwrap_or(&output.stdout);
            let expected = path_from_git_bytes(bytes)?;
            let expected = dunce::canonicalize(&expected).unwrap_or(expected);
            probe.env(EXPECTED, expected);
        }
        let marker = self.path("git-called");
        if mode == "fast" {
            // Record even failed Git invocations, instead of just checking that
            // discovery succeeds when Git is missing from PATH.
            let bin = self.path("bin");
            fs_err::create_dir_all(&bin)?;
            {
                use std::os::unix::fs::PermissionsExt;

                let git = bin.join("git");
                fs_err::write(
                    &git,
                    "#!/bin/sh\n: > \"$PREK_TEST_GIT_ROOT_CALLED\"\nexit 1\n",
                )?;
                fs_err::set_permissions(git, std::fs::Permissions::from_mode(0o755))?;
            }
            probe.env("PATH", bin).env(GIT_CALLED, &marker);
        }
        probe.assert().success();
        assert!(!marker.exists(), "Root discovery launched Git");
        Ok(())
    }
}

#[test]
fn root_without_subprocess_in_normal_and_nested_repositories() -> Result<()> {
    let fixture = Fixture::new()?;
    let repo = fixture.init("repo with spaces")?;
    let nested = repo.join("one/two/three");
    fs_err::create_dir_all(&nested)?;
    fixture.compare(&repo, &[], "fast")?;
    fixture.compare(&nested, &[], "fast")?;
    fixture.git(&nested).args(["init", "-q"]).assert().success();
    fixture.compare(&nested, &[], "fast")
}

#[test]
fn root_without_subprocess_in_linked_worktrees() -> Result<()> {
    let fixture = Fixture::new()?;
    let repo = fixture.init("repo")?;
    fixture.commit(&repo);
    let worktree = fixture.path("linked");
    fixture
        .git(&repo)
        .args(["worktree", "add", "--detach"])
        .arg(&worktree)
        .assert()
        .success();
    let nested = worktree.join("nested");
    fs_err::create_dir_all(&nested)?;
    fixture.compare(&nested, &[], "fast")?;
    let git_dir = repo.join(".git/worktrees/linked");
    fixture.compare(&worktree, &[("GIT_DIR", git_dir.into_os_string())], "fast")
}

#[test]
fn root_without_subprocess_in_submodules() -> Result<()> {
    let fixture = Fixture::new()?;
    let source = fixture.init("source")?;
    fixture.commit(&source);
    let repo = fixture.init("repo")?;
    fixture
        .git(&repo)
        .args(["-c", "protocol.file.allow=always", "submodule", "add"])
        .arg(&source)
        .arg("modules/sub")
        .assert()
        .success();
    let nested = repo.join("modules/sub/nested");
    fs_err::create_dir_all(&nested)?;
    fixture.compare(&nested, &[], "fast")
}

#[test]
fn root_respects_worktree_overrides() -> Result<()> {
    let fixture = Fixture::new()?;
    let repo = fixture.init("repo")?;
    let external = fixture.path("external");
    fs_err::create_dir_all(&external)?;
    fixture.compare(&repo, &[("GIT_DIR", OsString::from(".git"))], "fast")?;
    fixture.compare(
        &repo,
        &[("GIT_WORK_TREE", OsString::from("../external"))],
        "fallback",
    )?;
    fixture.compare(
        &external,
        &[("GIT_DIR", repo.join(".git").into_os_string())],
        "fallback",
    )?;
    fixture
        .git(&repo)
        .args(["config", "core.worktree", "../../external"])
        .assert()
        .success();
    fixture.compare(&repo, &[], "fallback")?;
    fixture.compare(&repo, &[("GIT_DIR", OsString::from(".git"))], "fallback")?;
    fixture.compare(
        &repo,
        &[
            ("GIT_DIR", OsString::from(".git")),
            ("GIT_WORK_TREE", OsString::from(".")),
        ],
        "fallback",
    )
}

#[test]
fn root_falls_back_for_worktree_config() -> Result<()> {
    let fixture = Fixture::new()?;
    let repo = fixture.init("repo")?;
    fixture.commit(&repo);
    let linked = fixture.path("linked");
    fixture
        .git(&repo)
        .args(["worktree", "add", "--detach"])
        .arg(&linked)
        .assert()
        .success();
    let external = fixture.path("external");
    fs_err::create_dir_all(&external)?;
    fixture
        .git(&repo)
        .args(["config", "extensions.worktreeConfig", "true"])
        .assert()
        .success();
    fixture
        .git(&repo)
        .args(["config", "--worktree", "core.worktree"])
        .arg(&external)
        .assert()
        .success();
    fixture.compare(&repo, &[], "fallback")?;
    fixture
        .git(&linked)
        .args(["config", "--worktree", "core.worktree"])
        .arg(&external)
        .assert()
        .success();
    fixture.compare(&linked, &[], "fallback")
}

#[test]
fn root_falls_back_for_quoted_config_and_duplicate_sections() -> Result<()> {
    let fixture = Fixture::new()?;
    let repo = fixture.init("repo")?;
    let external = fixture.path("external # \"quoted\" \\ directory ");
    fs_err::create_dir_all(&external)?;
    fixture
        .git(&repo)
        .args(["config", "core.worktree"])
        .arg(&external)
        .assert()
        .success();
    fixture.compare(&repo, &[], "fallback")?;

    let config = repo.join(".git/config");
    let contents = fs_err::read_to_string(&config)?;
    fs_err::write(
        &config,
        format!(
            "{contents}\n[CoRe]\n Bare = \"false\" # comment\n WorkTree = ..\n\
             [core \"other\"]\n bare = true\n",
        ),
    )?;
    fixture.compare(&repo, &[], "fallback")
}

#[test]
fn root_falls_back_for_includes_and_complex_config() -> Result<()> {
    let fixture = Fixture::new()?;
    let repo = fixture.init("repo")?;
    let config = repo.join(".git/config");
    let contents = fs_err::read_to_string(&config)?;
    let included = fixture.path("included");
    fs_err::write(&included, "[core]\nworktree = ..\n")?;
    fixture
        .git(&repo)
        .args(["config", "include.path"])
        .arg(&included)
        .assert()
        .success();
    fixture.compare(&repo, &[], "fallback")?;

    for (extra, mode) in [
        ("[core]\nworktree\n", "error"),
        ("[core]\nbare = true\nbare = false\n", "fallback"),
        ("[core]\nbare = off\n", "fallback"),
        ("[core] worktree = ..\n", "fallback"),
        ("[alias]\nexample = foo\\\n bar\n", "fallback"),
        ("[core]\nworktree = \"unterminated\n", "error"),
    ] {
        fs_err::write(&config, format!("{contents}\n{extra}"))?;
        fixture.compare(&repo, &[], mode)?;
    }
    fs_err::write(&config, format!("{contents}\n# {}\n", "x".repeat(65536)))?;
    fixture.compare(&repo, &[], "fallback")?;

    fs_err::write(&config, contents)?;
    fixture.compare(
        &repo,
        &[("GIT_CONFIG_GLOBAL", included.into_os_string())],
        "fast",
    )
}

#[test]
fn root_preserves_the_initial_work_tree_after_chdir() -> Result<()> {
    let fixture = Fixture::new()?;
    let repo = fixture.init("repo")?;
    let nested = repo.join("nested");
    fs_err::create_dir_all(&nested)?;
    let git_dir = repo.join(".git").into_os_string();
    fixture.compare(
        &repo,
        &[
            ("GIT_DIR", git_dir.clone()),
            (CHDIR, nested.into_os_string()),
        ],
        "fast",
    )?;
    let other = fixture.init("other")?;
    fixture.compare(
        &repo,
        &[("GIT_DIR", git_dir), (CHDIR, other.into_os_string())],
        "fallback",
    )
}

#[test]
fn root_falls_back_for_unsupported_overrides_and_errors() -> Result<()> {
    let fixture = Fixture::new()?;
    let repo = fixture.init("repo")?;
    fixture.compare(
        &repo,
        &[("GIT_COMMON_DIR", repo.join(".git").into_os_string())],
        "fallback",
    )?;
    fixture.compare(
        &repo,
        &[("GIT_WORK_TREE", OsString::from("missing"))],
        "fallback",
    )?;
    fixture.compare(
        &repo,
        &[
            ("GIT_CONFIG_COUNT", OsString::from("1")),
            ("GIT_CONFIG_KEY_0", OsString::from("core.worktree")),
            (
                "GIT_CONFIG_VALUE_0",
                fixture.path("external").into_os_string(),
            ),
        ],
        "fallback",
    )?;
    fixture.compare(&repo, &[("GIT_DIR", OsString::new())], "error")?;
    fixture.compare(
        &repo,
        &[("GIT_TEST_ASSUME_DIFFERENT_OWNER", OsString::from("1"))],
        "error",
    )?;
    fixture.compare(fixture.dir.path(), &[], "error")?;
    fixture.compare(&repo.join(".git"), &[], "error")?;
    fixture
        .git(&repo)
        .args(["config", "core.bare", "true"])
        .assert()
        .success();
    fixture.compare(&repo, &[], "error")
}

#[test]
fn root_falls_back_at_nonstandard_repository_markers() -> Result<()> {
    use std::os::unix::fs::symlink;

    let fixture = Fixture::new()?;
    let repo = fixture.init("repo")?;
    let nested = repo.join("nested");
    fs_err::create_dir_all(&nested)?;
    let marker = nested.join(".git");
    symlink(fixture.path("missing"), &marker)?;
    fixture.compare(&nested, &[], "fallback")?;
    fs_err::remove_file(&marker)?;
    fs_err::write(&marker, "not a git file\n")?;
    fixture.compare(&nested, &[], "error")?;
    fs_err::remove_file(marker)?;
    fixture
        .git(&nested)
        .args(["init", "--bare", "-q"])
        .assert()
        .success();
    fixture.compare(&nested, &[], "error")
}

#[test]
fn root_respects_discovery_ceilings() -> Result<()> {
    let fixture = Fixture::new()?;
    let repo = fixture.init("repo")?;
    let child = repo.join("child");
    fs_err::create_dir_all(&child)?;
    let ceiling = [("GIT_CEILING_DIRECTORIES", repo.clone().into_os_string())];
    fixture.compare(&repo, &ceiling, "fallback")?;
    fixture.compare(&child, &ceiling, "error")?;
    fixture.compare(
        &child,
        &[(
            "GIT_CEILING_DIRECTORIES",
            fixture.path("unrelated").into_os_string(),
        )],
        "fallback",
    )
}

#[cfg(unix)]
#[test]
fn root_without_subprocess_preserves_non_utf8_and_symlinked_paths() -> Result<()> {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::symlink;

    let fixture = Fixture::new()?;
    let repo = fixture.dir.path().join(OsStr::from_bytes(b"repo-\xff"));
    fs_err::create_dir_all(&repo)?;
    fixture.git(&repo).args(["init", "-q"]).assert().success();
    fixture.compare(&repo, &[], "fast")?;
    let link = fixture.path("link");
    symlink(&repo, &link)?;
    fixture.compare(&link, &[], "fast")
}
