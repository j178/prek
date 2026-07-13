use std::env::consts::EXE_EXTENSION;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::str;
use std::sync::Arc;

use anyhow::{Context, Result};
use prek_consts::env_vars::{EnvVars, EnvVarsRead};
use tracing::debug;

use crate::cli::reporter::HookInstallReporter;
use crate::cli::run::HookRunReporter;
use crate::hook::{Hook, InstallInfo, InstalledHook};
use crate::languages::LanguageBackend;
use crate::process::Cmd;
use crate::run::run_by_batch;
use crate::store::Store;

#[derive(Debug, Copy, Clone)]
pub(crate) struct R;

#[async_trait::async_trait(?Send)]
impl LanguageBackend for R {
    async fn install(
        &self,
        store: &Store,
        hook: Arc<Hook>,
        reporter: &HookInstallReporter,
    ) -> Result<InstalledHook> {
        let progress = reporter.on_install_start(&hook);

        let mut info = InstallInfo::new(&hook, &store.hooks_dir())?;

        debug!(%hook, target = %info.env_path.display(), "Installing R environment");

        let rscript = rscript_executable();
        let r_version = query_r_version(&rscript)
            .await
            .context("Failed to query R version")?;
        fs_err::tokio::create_dir_all(&info.env_path).await?;
        let activate = info.env_path.join("activate.R");

        if let Some(repo_path) = hook.repo_path() {
            fs_err::tokio::copy(repo_path.join("renv.lock"), info.env_path.join("renv.lock"))
                .await?;
            copy_dir_all(repo_path.join("renv"), info.env_path.join("renv")).await?;

            // Remote R hooks carry a renv project. Let renv/activate.R bootstrap
            // renv and choose the project library instead of overriding .libPaths.
            run_r_code(
                &rscript,
                &renv_project_install_code(repo_path),
                &hook.additional_dependencies,
                &info.env_path,
            )
            .await
            .context("Failed to install R hook environment")?;

            let env_dir = r_path(&info.env_path);
            fs_err::tokio::write(
                &activate,
                indoc::formatdoc! {r#"
                    suppressWarnings({{
                        old <- setwd({env_dir})
                        source("renv/activate.R")
                        setwd(old)
                        renv::load({env_dir})
                    }})
                "#},
            )
            .await?;
        } else if !hook.additional_dependencies.is_empty() {
            // Local hooks do not have a copied renv project to activate, and prek
            // intentionally avoids pre-commit's fake local project. Install deps
            // into a private library and expose it through R_PROFILE_USER.
            let lib_path = info.env_path.join("library");
            fs_err::tokio::create_dir_all(&lib_path).await?;

            run_r_code(
                &rscript,
                &additional_dependency_install_code(&info.env_path),
                &hook.additional_dependencies,
                hook.work_dir(),
            )
            .await
            .context("Failed to install R additional dependencies")?;

            let lib_dir = r_path(&lib_path);
            fs_err::tokio::write(&activate, format!(".libPaths(c({lib_dir}, .libPaths()))\n"))
                .await?;
        } else {
            fs_err::tokio::write(&activate, "").await?;
        }

        info.with_language_version(r_version)
            .with_toolchain(rscript);
        info.persist_env_path();

        reporter.on_install_complete(progress);

        Ok(InstalledHook::Installed {
            hook,
            info: Arc::new(info),
        })
    }

    async fn check_health(&self, info: &InstallInfo) -> Result<()> {
        let current = query_r_version(&rscript_executable())
            .await
            .context("Failed to query R version")?;

        if info.language_version != current {
            anyhow::bail!(
                "Hooks were installed for R version {}, but current R executable has version {}",
                info.language_version,
                current
            );
        }

        Ok(())
    }

    async fn run(
        &self,
        _store: &Store,
        hook: &InstalledHook,
        filenames: &[&Path],
        reporter: &HookRunReporter,
    ) -> Result<(i32, Vec<u8>)> {
        let progress = reporter.on_run_start(hook, filenames.len());

        let env_path = hook.env_path().expect("R must have env path");
        let activate = env_path.join("activate.R");
        let entry = r_hook_entry(hook)?;

        let run = async |batch: &[&Path]| {
            let mut cmd = Cmd::new(&entry[0]);
            cmd.current_dir(hook.work_dir())
                .args(&entry[1..])
                .env_remove(EnvVars::RENV_PROJECT)
                .env(EnvVars::R_PROFILE_USER, &activate)
                .stdin(Stdio::null());

            cmd.envs(&hook.env)
                .args(&hook.args)
                .file_args(batch)
                .check(false);

            let mut output = cmd
                .pty_output_with_sink(reporter.output_sink(progress))
                .await?;

            reporter.on_run_progress(progress, batch.len() as u64);

            output.stdout.extend(output.stderr);
            let code = output.status.code().unwrap_or(1);
            anyhow::Ok((code, output.stdout))
        };

        let results = run_by_batch(hook, filenames, &entry, run).await?;

        let mut combined_status = 0;
        let mut combined_output = Vec::new();

        for (code, output) in results {
            combined_status |= code;
            combined_output.extend(output);
        }

        reporter.on_run_complete(progress);

        Ok((combined_status, combined_output))
    }
}

async fn run_r_code(rscript: &Path, code: &str, args: &[String], cwd: &Path) -> Result<()> {
    let script_dir = tempfile::tempdir()?;
    let script_path = script_dir.path().join("script.R");
    fs_err::tokio::write(
        &script_path,
        indoc::formatdoc! {r#"
            options(
                install.packages.compile.from.source = "never",
                pkgType = "binary"
            )
            {code}
        "#},
    )
    .await?;

    Cmd::new(rscript)
        .current_dir(cwd)
        .arg("--vanilla")
        .arg(script_path)
        .args(args)
        .env_remove(EnvVars::RENV_PROJECT)
        .check(true)
        .output()
        .await?;
    Ok(())
}

async fn query_r_version(rscript: &Path) -> Result<semver::Version> {
    let output = Cmd::new(rscript)
        .arg("--vanilla")
        .arg("-e")
        .arg("cat(as.character(getRversion()))")
        .env_remove(EnvVars::RENV_PROJECT)
        .check(true)
        .output()
        .await?;
    let version = str::from_utf8(&output.stdout)?;
    let version = version.trim();
    semver::Version::parse(version)
        .with_context(|| format!("Failed to parse R version `{version}`"))
}

fn renv_project_install_code(repo_path: &Path) -> String {
    let repo_dir = r_path(repo_path);
    indoc::formatdoc! {r#"
        repo_dir <- {repo_dir}
        options(
            repos = c(CRAN = "https://cran.rstudio.com"),
            renv.consent = TRUE
        )

        source("renv/activate.R")
        renv::restore()

        path_desc <- file.path(repo_dir, "DESCRIPTION")
        is_package <- tryCatch({{
            suppressWarnings(desc <- read.dcf(path_desc))
            "Package" %in% colnames(desc)
        }}, error = function(...) FALSE)
        if (is_package) {{
            renv::install(repo_dir)
        }}

        deps <- commandArgs(trailingOnly = TRUE)
        if (length(deps)) {{
            setwd(repo_dir)
            renv::install(deps)
        }}
    "#}
}

fn additional_dependency_install_code(env_path: &Path) -> String {
    let env_dir = r_path(env_path);
    indoc::formatdoc! {r#"
        env_dir <- {env_dir}
        lib_dir <- file.path(env_dir, "library")
        dir.create(lib_dir, recursive = TRUE, showWarnings = FALSE)
        .libPaths(c(lib_dir, .libPaths()))
        options(
            repos = c(CRAN = "https://cran.rstudio.com"),
            renv.consent = TRUE
        )
        if (!requireNamespace("renv", quietly = TRUE)) {{
            install.packages("renv", lib = lib_dir, type = .Platform$pkgType)
        }}

        deps <- commandArgs(trailingOnly = TRUE)
        renv::install(deps, library = lib_dir)
    "#}
}

fn r_hook_entry(hook: &InstalledHook) -> Result<Vec<String>> {
    let entry = hook.entry.expect_direct().split()?;
    validate_r_entry(&entry)?;

    let mut cmd = Vec::with_capacity(entry.len() + 4);
    cmd.push(entry[0].clone());
    cmd.extend(
        [
            "--no-save",
            "--no-restore",
            "--no-site-file",
            "--no-environ",
        ]
        .map(String::from),
    );

    if let Some(repo_path) = hook.repo_path() {
        if entry[1] == "-e" {
            cmd.extend(entry[1..].iter().cloned());
        } else {
            cmd.push(repo_path.join(&entry[1]).to_string_lossy().into_owned());
        }
    } else {
        cmd.extend(entry[1..].iter().cloned());
    }

    Ok(cmd)
}

fn validate_r_entry(entry: &[String]) -> Result<()> {
    if entry.len() < 2 || entry[0] != "Rscript" {
        anyhow::bail!("entry must start with `Rscript`");
    }

    if entry[1] == "-e" {
        if entry.len() > 3 {
            anyhow::bail!("You can supply at most one expression");
        }
    } else if entry.len() > 2 {
        anyhow::bail!(
            "The only valid syntax is `Rscript -e {{expr}}` or `Rscript path/to/hook/script`"
        );
    }

    Ok(())
}

fn rscript_executable() -> PathBuf {
    if let Some(r_home) = EnvVars.var_os(EnvVars::R_HOME) {
        PathBuf::from(r_home)
            .join("bin/Rscript")
            .with_extension(EXE_EXTENSION)
    } else {
        Path::new("Rscript").with_extension(EXE_EXTENSION)
    }
}

fn r_path(path: &Path) -> String {
    serde_json::to_string(path.to_string_lossy().as_ref()).expect("path string must serialize")
}

async fn copy_dir_all(src: PathBuf, dst: PathBuf) -> Result<()> {
    tokio::task::spawn_blocking(move || copy_dir_all_blocking(&src, &dst)).await?
}

fn copy_dir_all_blocking(src: &Path, dst: &Path) -> Result<()> {
    for entry in walkdir::WalkDir::new(src) {
        let entry = entry?;
        let target = dst.join(entry.path().strip_prefix(src)?);
        if entry.file_type().is_dir() {
            fs_err::create_dir_all(target)?;
        } else if entry.file_type().is_file() {
            if let Some(parent) = target.parent() {
                fs_err::create_dir_all(parent)?;
            }
            fs_err::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{r_path, validate_r_entry};

    #[test]
    fn r_path_quotes_lossy_path() {
        assert_eq!(
            r_path(Path::new(r#"C:\tmp\"quoted""#)),
            r#""C:\\tmp\\\"quoted\"""#
        );
    }

    #[test]
    fn validate_r_entry_allows_supported_forms() {
        validate_r_entry(&["Rscript".into(), "-e".into(), "1+1".into()]).unwrap();
        validate_r_entry(&["Rscript".into(), "hook.R".into()]).unwrap();
    }

    #[test]
    fn validate_r_entry_rejects_options_for_file_entries() {
        let err =
            validate_r_entry(&["Rscript".into(), "--vanilla".into(), "hook.R".into()]).unwrap_err();
        assert!(
            err.to_string()
                .contains("The only valid syntax is `Rscript -e {expr}`")
        );
    }

    #[test]
    fn r_path_preserves_spaces() {
        assert_eq!(r_path(Path::new("/tmp/r env")), r#""/tmp/r env""#);
    }
}
