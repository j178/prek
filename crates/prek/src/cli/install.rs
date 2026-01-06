use std::fmt::Write as _;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use bstr::ByteSlice;
use owo_colors::OwoColorize;
use prek_consts::{PRE_COMMIT_CONFIG_YAML, PRE_COMMIT_CONFIG_YML, PREK_TOML};
use same_file::is_same_file;

use crate::cli::reporter::{HookInitReporter, HookInstallReporter};
use crate::cli::run;
use crate::cli::run::{SelectorSource, Selectors};
use crate::cli::{ExitStatus, HookType};
use crate::config::load_config;
use crate::fs::{CWD, Simplified};
use crate::git::{GIT_ROOT, git_cmd};
use crate::printer::Printer;
use crate::store::Store;
use crate::workspace::{Project, Workspace};
use crate::{git, warn_user};

#[allow(clippy::fn_params_excessive_bools)]
pub(crate) async fn install(
    store: &Store,
    config: Option<PathBuf>,
    includes: Vec<String>,
    skips: Vec<String>,
    hook_types: Vec<HookType>,
    install_hook_environments: bool,
    overwrite: bool,
    allow_missing_config: bool,
    refresh: bool,
    printer: Printer,
    git_dir: Option<&Path>,
) -> Result<ExitStatus> {
    if git_dir.is_none() && git::has_hooks_path_set().await? {
        anyhow::bail!(
            "Cowardly refusing to install hooks with `core.hooksPath` set.\nhint: Run these commands to remove core.hooksPath:\nhint:   {}\nhint:   {}",
            "git config --unset-all --local core.hooksPath".cyan(),
            "git config --unset-all --global core.hooksPath".cyan()
        );
    }

    let project = Project::discover(config.as_deref(), &CWD).ok();
    let hook_types = get_hook_types(hook_types, project.as_ref(), config.as_deref());

    let hooks_path = if let Some(dir) = git_dir {
        dir.join("hooks")
    } else {
        git::get_git_common_dir().await?.join("hooks")
    };
    fs_err::create_dir_all(&hooks_path)?;

    let selectors = if let Some(project) = &project {
        Some(Selectors::load(&includes, &skips, project.path())?)
    } else if !includes.is_empty() || !skips.is_empty() {
        anyhow::bail!("Cannot use `--include` or `--skip` outside of a git repository");
    } else {
        None
    };

    for hook_type in hook_types {
        install_hook_script(
            project.as_ref(),
            config.clone(),
            selectors.as_ref(),
            hook_type,
            &hooks_path,
            overwrite,
            allow_missing_config,
            printer,
        )?;
    }

    if install_hook_environments {
        install_hooks(store, config, includes, skips, refresh, printer).await?;
    }

    Ok(ExitStatus::Success)
}

pub(crate) async fn install_hooks(
    store: &Store,
    config: Option<PathBuf>,
    includes: Vec<String>,
    skips: Vec<String>,
    refresh: bool,
    printer: Printer,
) -> Result<ExitStatus> {
    let workspace_root = Workspace::find_root(config.as_deref(), &CWD)?;
    let selectors = Selectors::load(&includes, &skips, &workspace_root)?;
    let mut workspace =
        Workspace::discover(store, workspace_root, config, Some(&selectors), refresh)?;

    let reporter = HookInitReporter::from(printer);
    let _lock = store.lock_async().await?;

    let hooks = workspace
        .init_hooks(store, Some(&reporter))
        .await
        .context("Failed to init hooks")?;
    let filtered_hooks: Vec<_> = hooks
        .into_iter()
        .filter(|h| selectors.matches_hook(h))
        .map(Arc::new)
        .collect();

    let reporter = HookInstallReporter::from(printer);
    run::install_hooks(filtered_hooks, store, &reporter).await?;

    Ok(ExitStatus::Success)
}

fn get_hook_types(
    mut hook_types: Vec<HookType>,
    project: Option<&Project>,
    config: Option<&Path>,
) -> Vec<HookType> {
    if !hook_types.is_empty() {
        return hook_types;
    }

    hook_types = if let Some(project) = project {
        project
            .config()
            .default_install_hook_types
            .clone()
            .unwrap_or_default()
    } else {
        let fallbacks = [PREK_TOML, PRE_COMMIT_CONFIG_YAML, PRE_COMMIT_CONFIG_YML]
            .iter()
            .map(Path::new)
            .filter(|p| p.exists());
        config
            .into_iter()
            .chain(fallbacks)
            .next()
            .and_then(|p| load_config(p).ok())
            .and_then(|cfg| cfg.default_install_hook_types.clone())
            .unwrap_or_default()
    };
    if hook_types.is_empty() {
        hook_types = vec![HookType::PreCommit];
    }

    hook_types
}

fn install_hook_script(
    project: Option<&Project>,
    config: Option<PathBuf>,
    selectors: Option<&Selectors>,
    hook_type: HookType,
    hooks_path: &Path,
    overwrite: bool,
    skip_on_missing_config: bool,
    printer: Printer,
) -> Result<()> {
    let hook_path = hooks_path.join(hook_type.as_str());

    if hook_path.try_exists()? {
        if overwrite {
            writeln!(
                printer.stdout(),
                "Overwriting existing hook at `{}`",
                hook_path.user_display().cyan()
            )?;
        } else {
            if !is_our_script(&hook_path)? {
                let legacy_path = format!("{}.legacy", hook_path.display());
                fs_err::rename(&hook_path, &legacy_path)?;
                writeln!(
                    printer.stdout(),
                    "Hook already exists at `{}`, moved it to `{}`",
                    hook_path.user_display().cyan(),
                    legacy_path.user_display().yellow()
                )?;
            }
        }
    }

    let mut args = vec!["hook-impl".to_string()];

    // Add include/skip selectors.
    if let Some(selectors) = selectors {
        for include in selectors.includes() {
            args.push(include.as_normalized_flag());
        }

        // Find any skip selectors from environment variables.
        if let Some(env_var) = selectors.skips().iter().find_map(|skip| {
            if let SelectorSource::EnvVar(var) = skip.source() {
                Some(var)
            } else {
                None
            }
        }) {
            warn_user!(
                "Skip selectors from environment variables `{}` are ignored during installing hooks.",
                env_var.cyan()
            );
        }

        for skip in selectors.skips() {
            if matches!(skip.source(), SelectorSource::CliFlag(_)) {
                args.push(skip.as_normalized_flag());
            }
        }
    }

    args.push(format!("--hook-type={}", hook_type.as_str()));

    let mut hint = format!("prek installed at `{}`", hook_path.user_display().cyan());

    // Prefer explicit config path if given (non-workspace mode).
    // Otherwise, use the config path from the discovered project (workspace mode).
    // If neither is available, don't pass a config path (let prek find it). In this case,
    // we're different with `pre-commit` which always sets `--config=.pre-commit-config.yaml`.
    if let Some(config) = config {
        args.push(format!(r#"--config="{}""#, config.display()));

        write!(hint, " with specified config `{}`", config.display().cyan())?;
    } else if let Some(project) = project {
        let git_root = GIT_ROOT.as_ref()?;
        let project_path = project.path();
        let relative_path = project_path.strip_prefix(git_root).unwrap_or(project_path);
        if !relative_path.as_os_str().is_empty() {
            args.push(format!(r#"--cd="{}""#, relative_path.display()));
        }

        // Show workspace path if it's not the root project.
        if project_path != git_root {
            writeln!(hint, " for workspace `{}`", project_path.display().cyan())?;
            write!(
                hint,
                "\n{} this hook installed for `{}` only; run `prek install` from `{}` to install for the entire repo.",
                "hint:".bold().yellow(),
                project_path.display().cyan(),
                git_root.display().cyan()
            )?;
        }
    }

    if skip_on_missing_config {
        args.push("--skip-on-missing-config".to_string());
    }

    args.push(format!("--script-version={CUR_SCRIPT_VERSION}"));

    let prek = std::env::current_exe()?;
    let prek = prek.simplified_display().to_string();
    let hook_script = HOOK_TMPL
        .replace(
            "[SHEBANG]",
            if cfg!(windows) {
                "#!/bin/sh"
            } else {
                "#!/usr/bin/env bash"
            },
        )
        .replace("[PREK_ARGS]", &args.join(" "))
        .replace("[PREK_PATH]", &format!(r#""{prek}""#));

    fs_err::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&hook_path)?
        .write_all(hook_script.as_bytes())?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let mut perms = hook_path.metadata()?.permissions();
        perms.set_mode(0o755);
        fs_err::set_permissions(&hook_path, perms)?;
    }

    writeln!(printer.stdout(), "{hint}")?;

    Ok(())
}

/// The version of the hook script. Increment this when the script changes in a way that
/// requires re-installation.
pub(crate) static CUR_SCRIPT_VERSION: usize = 4;

static HOOK_TMPL: &str = r#"[SHEBANG]
# File generated by prek: https://github.com/j178/prek
# ID: 182c10f181da4464a3eec51b83331688

ARGS=([PREK_ARGS])

HERE="$(cd "$(dirname "$0")" && pwd)"
ARGS+=(--hook-dir "$HERE" -- "$@")
PREK=[PREK_PATH]

# Check if the full path to prek is executable, otherwise fallback to PATH
if [ ! -x "$PREK" ]; then
    PREK="prek"
fi

exec "$PREK" "${ARGS[@]}"

"#;

static PRIOR_HASHES: &[&str] = &[];

// Use a different hash for each change to the script.
// Use a different hash from `pre-commit` since our script is different.
static CURRENT_HASH: &str = "182c10f181da4464a3eec51b83331688";

/// Checks if the script contains any of the hashes that `prek` has used in the past.
fn is_our_script(hook_path: &Path) -> Result<bool> {
    let content = fs_err::read_to_string(hook_path)?;
    Ok(std::iter::once(CURRENT_HASH)
        .chain(PRIOR_HASHES.iter().copied())
        .any(|hash| content.contains(hash)))
}

pub(crate) async fn uninstall(
    config: Option<PathBuf>,
    hook_types: Vec<HookType>,
    printer: Printer,
) -> Result<ExitStatus> {
    let project = Project::discover(config.as_deref(), &CWD).ok();
    let hooks_path = git::get_git_common_dir().await?.join("hooks");

    for hook_type in get_hook_types(hook_types, project.as_ref(), config.as_deref()) {
        let hook_path = hooks_path.join(hook_type.as_str());
        let legacy_path = hooks_path.join(format!("{}.legacy", hook_type.as_str()));

        if !hook_path.try_exists()? {
            writeln!(
                printer.stderr(),
                "`{}` does not exist, skipping.",
                hook_path.user_display().cyan()
            )?;
        } else if !is_our_script(&hook_path)? {
            writeln!(
                printer.stderr(),
                "`{}` is not managed by prek, skipping.",
                hook_path.user_display().cyan()
            )?;
        } else {
            fs_err::remove_file(&hook_path)?;
            writeln!(
                printer.stdout(),
                "Uninstalled `{}`",
                hook_type.as_str().cyan()
            )?;

            if legacy_path.try_exists()? {
                fs_err::rename(&legacy_path, &hook_path)?;
                writeln!(
                    printer.stdout(),
                    "Restored previous hook to `{}`",
                    hook_path.user_display().cyan()
                )?;
            }
        }
    }

    Ok(ExitStatus::Success)
}

pub(crate) async fn init_template_dir(
    store: &Store,
    directory: PathBuf,
    config: Option<PathBuf>,
    hook_types: Vec<HookType>,
    requires_config: bool,
    refresh: bool,
    printer: Printer,
) -> Result<ExitStatus> {
    install(
        store,
        config,
        vec![],
        vec![],
        hook_types,
        false,
        true,
        !requires_config,
        refresh,
        printer,
        Some(&directory),
    )
    .await?;

    let output = git_cmd("git config")?
        .arg("config")
        .arg("init.templateDir")
        .check(false)
        .output()
        .await?;
    let template_dir = String::from_utf8_lossy(output.stdout.trim()).to_string();

    if template_dir.is_empty() || !is_same_file(&directory, &template_dir)? {
        warn_user!(
            "git config `init.templateDir` not set to the target directory, try `{}`",
            format!(
                "git config --global init.templateDir '{}'",
                directory.display()
            )
            .cyan()
        );
    }

    Ok(ExitStatus::Success)
}
