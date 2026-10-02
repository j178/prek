use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};

use crate::cli::ExitStatus;
use crate::cli::reporter::{HookInitReporter, HookInstallReporter};
use crate::cli::run::{InstallCache, Selectors, install_hooks};
use crate::fs::CWD;
use crate::printer::Printer;
use crate::store::Store;
use crate::workspace::{HookInitFilters, Workspace};

pub(crate) async fn exec(
    store: &Store,
    config: Option<PathBuf>,
    selector: String,
    command: Vec<OsString>,
    refresh: bool,
    printer: Printer,
) -> Result<ExitStatus> {
    let workspace_root = Workspace::find_root(config.as_deref(), &CWD)?;
    let selectors = Selectors::from_include(&selector, &workspace_root)?;
    let workspace = Workspace::discover(store, workspace_root, config, Some(&selectors), refresh)?;

    let init_reporter = HookInitReporter::new(printer);
    let lock = store.lock_async().await?;
    store.track_configs(workspace.config_files())?;

    let hooks = workspace
        .init_hooks(
            store,
            HookInitFilters::new(Some(&selectors), None),
            Some(&init_reporter),
        )
        .await
        .context("Failed to init hooks")?
        .into_iter()
        .filter(|hook| selectors.matches_hook(hook))
        .map(Arc::new)
        .collect::<Vec<_>>();

    let hook = match hooks.as_slice() {
        [] => anyhow::bail!("Hook selector `{selector}` did not match any hooks"),
        [hook] => Arc::clone(hook),
        _ => {
            let matches = hooks
                .iter()
                .map(|hook| format!("  - {}", hook.full_id()))
                .collect::<Vec<_>>()
                .join("\n");
            anyhow::bail!(
                "Hook selector `{selector}` matched multiple hooks:\n{matches}\nUse a `project-path:hook-id` selector to select one hook"
            );
        }
    };
    hook.language.ensure_exec_supported(&hook)?;

    let install_reporter = HookInstallReporter::new(printer);
    let mut install_cache = InstallCache::new();
    let installed_hooks =
        install_hooks(vec![hook], store, &install_reporter, &mut install_cache).await?;
    install_reporter.on_complete();
    let installed_hook = installed_hooks
        .into_iter()
        .next()
        .context("Failed to prepare the selected hook environment")?;
    drop(lock);

    let status = installed_hook
        .language
        .exec(store, &installed_hook, &CWD, &command)
        .await?;
    Ok(external_exit_status(status))
}

fn external_exit_status(status: std::process::ExitStatus) -> ExitStatus {
    let code = status.code().and_then(|code| u8::try_from(code).ok());

    #[cfg(unix)]
    let code = code.or_else(|| {
        use std::os::unix::process::ExitStatusExt;

        status
            .signal()
            .and_then(|signal| u8::try_from(128 + signal).ok())
    });

    code.unwrap_or(1).into()
}
