use std::env;
use std::fmt::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::{Context, Result};
use tempfile::tempdir_in;

use crate::cli::{ExitStatus, GlobalArgs, TryRepoArgs};
use crate::config::{self, HookOptions, Repo};
use crate::git;
use crate::printer::Printer;
use crate::store::{STORE, Store};
use crate::warn_user;

async fn get_repo_and_rev(
    repo: &Path,
    rev: Option<&str>,
    tmpdir: &Path,
) -> Result<(PathBuf, String)> {
    let repo = std::fs::canonicalize(repo)?;

    if let Some(rev) = rev {
        return Ok((repo, rev.to_string()));
    }

    let head_rev = git::git_cmd("get head rev")?
        .arg("rev-parse")
        .arg("HEAD")
        .current_dir(&repo)
        .output()
        .await?
        .stdout;
    let head_rev = String::from_utf8_lossy(&head_rev).trim().to_string();

    if git::has_diff("HEAD", &repo).await? {
        warn_user!("Creating temporary repo with uncommitted changes...");

        let shadow = tmpdir.join("shadow-repo");
        git::git_cmd("clone shadow repo")?
            .arg("clone")
            .arg(&repo)
            .arg(&shadow)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await?;
        git::git_cmd("checkout shadow repo")?
            .arg("checkout")
            .arg(&head_rev)
            .arg("-b")
            .arg("_pc_tmp")
            .current_dir(&shadow)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await?;

        let index_path = shadow.join(".git/index");
        let objects_path = shadow.join(".git/objects");

        let staged_files = git::get_staged_files(&repo).await?;
        if !staged_files.is_empty() {
            let mut add_cmd = git::git_cmd("add staged files to shadow")?;
            add_cmd
                .arg("add")
                .arg("--")
                .args(&staged_files)
                .current_dir(&repo)
                .env("GIT_INDEX_FILE", &index_path)
                .env("GIT_OBJECT_DIRECTORY", &objects_path)
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            add_cmd.status().await?;
        }

        let mut add_u_cmd = git::git_cmd("add unstaged to shadow")?;
        add_u_cmd
            .arg("add")
            .arg("-u")
            .current_dir(&repo)
            .env("GIT_INDEX_FILE", &index_path)
            .env("GIT_OBJECT_DIRECTORY", &objects_path)
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        add_u_cmd.status().await?;

        git::commit(&shadow, "temp commit for try-repo").await?;

        let new_rev = git::git_cmd("get shadow head")?
            .arg("rev-parse")
            .arg("HEAD")
            .current_dir(&shadow)
            .output()
            .await?
            .stdout;
        let new_rev = String::from_utf8_lossy(&new_rev).trim().to_string();

        return Ok((shadow, new_rev));
    }

    Ok((repo, head_rev))
}

pub(crate) async fn try_repo(
    args: TryRepoArgs,
    globals: &GlobalArgs,
    printer: Printer,
) -> Result<ExitStatus> {
    let store = STORE.as_ref()?;
    let scratch_dir = store.scratch_path();
    fs_err::tokio::create_dir_all(&scratch_dir).await?;
    let tempdir = tempdir_in(scratch_dir)?;

    let (repo_path, rev) = get_repo_and_rev(&args.repo, args.r#ref.as_deref(), tempdir.path())
        .await
        .context("Failed to determine repository and revision")?;

    let store = Store::from_path(tempdir.path().join("store"));
    let repo_clone_path = store
        .clone_repo(
            &config::RemoteRepo {
                repo: repo_path.to_string_lossy().to_string(),
                rev: rev.clone(),
                hooks: vec![],
            },
            None,
        )
        .await?;

    let manifest = config::read_manifest(&repo_clone_path.join(constants::MANIFEST_FILE))?;

    let hooks: Vec<config::RemoteHook> = if let Some(hook_id) = &args.hook {
        vec![config::RemoteHook {
            id: hook_id.clone(),
            name: None,
            entry: None,
            language: None,
            options: HookOptions::default(),
        }]
    } else {
        manifest
            .hooks
            .into_iter()
            .map(|h| config::RemoteHook {
                id: h.id,
                name: None,
                entry: None,
                language: None,
                options: HookOptions::default(),
            })
            .collect()
    };

    let config = config::Config {
        repos: vec![Repo::Remote(config::RemoteRepo {
            repo: repo_path.to_string_lossy().to_string(),
            rev,
            hooks,
        })],
        default_install_hook_types: None,
        default_language_version: None,
        default_stages: None,
        files: None,
        exclude: None,
        fail_fast: None,
        minimum_prek_version: None,
        ci: None,
    };

    let config_s = serde_yaml::to_string(&config)?;
    let config_filename = tempdir.path().join(constants::CONFIG_FILE);
    fs_err::tokio::write(&config_filename, &config_s).await?;

    let mut stdout = printer.stdout();
    writeln!(stdout, "{}", "=".repeat(79))?;
    writeln!(stdout, "Using config:")?;
    writeln!(stdout, "{}", "=".repeat(79))?;
    write!(stdout, "{config_s}")?;
    writeln!(stdout, "{}", "=".repeat(79))?;

    // `try-repo` needs a git repository to run in.
    let run_in_dir = tempdir.path().join("run-in");
    fs_err::tokio::create_dir_all(&run_in_dir).await?;
    git::git_cmd("init for try-repo")?
        .arg("init")
        .current_dir(&run_in_dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await?;

    let mut run_args = args.run_args;

    let original_cwd = env::current_dir()?;
    let files_to_run: Vec<String> = run_args
        .files
        .iter()
        .map(|f| {
            let absolute_path = original_cwd.join(f);
            absolute_path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .to_string()
        })
        .collect();

    for file_path_str in &run_args.files {
        let source_path = original_cwd.join(file_path_str);
        if source_path.exists() {
            let target_path = run_in_dir.join(source_path.file_name().unwrap());
            fs_err::copy(source_path, &target_path)?;
        }
    }

    run_args.files = files_to_run;

    // Create a dummy file to run against if no files are provided.
    if run_args.files.is_empty() && !run_args.all_files {
        let dummy_file = "dummy-file";
        fs_err::tokio::write(run_in_dir.join(dummy_file), "").await?;
        git::git_cmd("add dummy file")?
            .arg("add")
            .arg(dummy_file)
            .current_dir(&run_in_dir)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await?;
    } else if !run_args.files.is_empty() {
        git::git_cmd("add copied files")?
            .arg("add")
            .args(&run_args.files)
            .current_dir(&run_in_dir)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await?;
    }

    env::set_current_dir(&run_in_dir)?;

    let result = crate::cli::run(
        Some(config_filename),
        vec![], // includes
        vec![], // skips
        run_args.hook_stage,
        run_args.from_ref,
        run_args.to_ref,
        run_args.all_files,
        run_args.files,
        run_args.directory,
        run_args.last_commit,
        run_args.show_diff_on_failure,
        run_args.dry_run,
        true, // refresh
        run_args.extra,
        globals.verbose > 0,
        printer,
    )
    .await;

    env::set_current_dir(original_cwd)?;

    result
}
