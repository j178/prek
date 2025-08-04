use std::fmt::Write as _;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::Result;
use indoc::indoc;
use owo_colors::OwoColorize;
use same_file::is_same_file;

use crate::cli::reporter::{HookInitReporter, HookInstallReporter};
use crate::cli::run;
use crate::cli::{ExitStatus, HookType};
use crate::fs::Simplified;
use crate::git;
use crate::git::git_cmd;
use crate::hook::Project;
use crate::printer::Printer;
use crate::store::Store;

pub(crate) async fn install(
    config: Option<PathBuf>,
    hook_types: Vec<HookType>,
    install_hook_environments: bool,
    overwrite: bool,
    allow_missing_config: bool,
    printer: Printer,
    git_dir: Option<&Path>,
) -> Result<ExitStatus> {
    if git_dir.is_none() && git::has_hooks_path_set().await? {
        writeln!(
            printer.stderr(),
            indoc::indoc! {"
                Cowardly refusing to install hooks with `core.hooksPath` set.
                hint: `git config --unset-all core.hooksPath` to fix this.
            "}
        )?;
        return Ok(ExitStatus::Failure);
    }

    let project = Project::from_config_file(config.clone()).ok();
    let hook_types = get_hook_types(project.as_ref(), hook_types);

    let hooks_path = if let Some(dir) = git_dir {
        dir.join("hooks")
    } else {
        git::get_git_common_dir().await?.join("hooks")
    };
    fs_err::create_dir_all(&hooks_path)?;

    for hook_type in hook_types {
        install_hook_script(
            project.as_ref(),
            hook_type,
            &hooks_path,
            overwrite,
            allow_missing_config,
            printer,
        )?;
    }

    if install_hook_environments {
        install_hooks(config, printer).await?;
    }

    Ok(ExitStatus::Success)
}

pub(crate) async fn install_hooks(config: Option<PathBuf>, printer: Printer) -> Result<ExitStatus> {
    let mut project = Project::from_config_file(config)?;
    let store = Store::from_settings()?.init()?;
    let _lock = store.lock_async().await?;

    let reporter = HookInitReporter::from(printer);
    let hooks = project.init_hooks(&store, Some(&reporter)).await?;
    let reporter = HookInstallReporter::from(printer);
    run::install_hooks(hooks, &store, &reporter).await?;

    Ok(ExitStatus::Success)
}

fn get_hook_types(project: Option<&Project>, hook_types: Vec<HookType>) -> Vec<HookType> {
    let mut hook_types = if hook_types.is_empty() {
        if let Some(project) = project {
            project
                .config()
                .default_install_hook_types
                .clone()
                .unwrap_or_default()
        } else {
            vec![]
        }
    } else {
        hook_types
    };
    if hook_types.is_empty() {
        hook_types = vec![HookType::PreCommit];
    }

    hook_types
}

fn install_hook_script(
    project: Option<&Project>,
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
                "Overwriting existing hook at {}",
                hook_path.user_display().cyan()
            )?;
        } else {
            if !is_our_script(&hook_path)? {
                let legacy_path = format!("{}.legacy", hook_path.display());
                fs_err::rename(&hook_path, &legacy_path)?;
                writeln!(
                    printer.stdout(),
                    "Hook already exists at {}, move it to {}.",
                    hook_path.user_display().cyan(),
                    legacy_path.user_display().yellow()
                )?;
            }
        }
    }

    let mut args = vec![
        "hook-impl".to_string(),
        format!("--hook-type={}", hook_type.as_str()),
    ];
    if let Some(project) = project {
        args.push(format!(
            r#"--config="{}""#,
            project.config_file().user_display()
        ));
    }
    if skip_on_missing_config {
        args.push("--skip-on-missing-config".to_string());
    }

    let prefligit = std::env::current_exe()?;
    let prefligit = prefligit.simplified().display().to_string();
    let hook_script = HOOK_TMPL
        .replace("ARGS=(hook-impl)", &format!("ARGS=({})", args.join(" ")))
        .replace(
            r#"PREFLIGIT="prefligit""#,
            &format!(r#"PREFLIGIT="{prefligit}""#),
        );
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

    writeln!(
        printer.stdout(),
        "prefligit installed at {}",
        hook_path.user_display().cyan()
    )?;

    Ok(())
}

static HOOK_TMPL: &str = indoc! { r#"
#!/usr/bin/env bash
# File generated by prefligit: https://github.com/j178/prefligit
# ID: 182c10f181da4464a3eec51b83331688

ARGS=(hook-impl)

HERE="$(cd "$(dirname "$0")" && pwd)"
ARGS+=(--hook-dir "$HERE" -- "$@")
PREFLIGIT="prefligit"

exec "$PREFLIGIT" "${ARGS[@]}"

"# };

static PRIOR_HASHES: &[&str] = &[];

// Use a different hash for each change to the script.
// Use a different hash from `pre-commit` since our script is different.
static CURRENT_HASH: &str = "182c10f181da4464a3eec51b83331688";

/// Checks if the script contains any of the hashes that `prefligit` has used in the past.
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
    let project = Project::from_config_file(config).ok();
    for hook_type in get_hook_types(project.as_ref(), hook_types) {
        let hooks_path = git::get_git_common_dir().await?.join("hooks");
        let hook_path = hooks_path.join(hook_type.as_str());
        let legacy_path = hooks_path.join(format!("{}.legacy", hook_type.as_str()));

        if !hook_path.try_exists()? {
            writeln!(
                printer.stderr(),
                "{} does not exist, skipping.",
                hook_path.user_display().cyan()
            )?;
        } else if !is_our_script(&hook_path)? {
            writeln!(
                printer.stderr(),
                "{} is not managed by prefligit, skipping.",
                hook_path.user_display().cyan()
            )?;
        } else {
            fs_err::remove_file(&hook_path)?;
            writeln!(
                printer.stdout(),
                "Uninstalled {}",
                hook_type.as_str().cyan()
            )?;

            if legacy_path.try_exists()? {
                fs_err::rename(&legacy_path, &hook_path)?;
                writeln!(
                    printer.stdout(),
                    "Restored previous hook to {}",
                    hook_path.user_display().cyan()
                )?;
            }
        }
    }

    Ok(ExitStatus::Success)
}

pub(crate) async fn init_template_dir(
    directory: PathBuf,
    config: Option<PathBuf>,
    hook_types: Vec<HookType>,
    requires_config: bool,
    printer: Printer,
) -> Result<ExitStatus> {
    install(
        config,
        hook_types,
        false,
        true,
        !requires_config,
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
    let template_dir = String::from_utf8_lossy(&output.stdout).trim().to_string();

    if template_dir.is_empty() || !is_same_file(&directory, Path::new(&template_dir))? {
        writeln!(
            printer.stderr(),
            "{}",
            indoc::formatdoc! {"
                `init.templateDir` not set to the target directory
                try `git config --global init.templateDir '{directory}'`?
            ", directory = directory.user_display().cyan() }
        )?;
    }

    Ok(ExitStatus::Success)
}
