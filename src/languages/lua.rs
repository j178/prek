use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use anyhow::{Context, Result};
use prek_consts::env_vars::EnvVars;
use semver::Version;
use tracing::debug;

use crate::cli::reporter::HookInstallReporter;
use crate::hook::{Hook, InstallInfo, InstalledHook};
use crate::languages::LanguageImpl;
use crate::process::Cmd;
use crate::run::{prepend_paths, run_by_batch};
use crate::store::Store;

#[derive(Debug, Copy, Clone)]
pub(crate) struct Lua;

pub(crate) struct LuaInfo {
    pub(crate) version: Version,
    pub(crate) executable: std::path::PathBuf,
}

pub(crate) async fn query_lua_info() -> Result<LuaInfo> {
    let stdout = Cmd::new("lua", "get lua version")
        .arg("-v")
        .check(true)
        .output()
        .await?
        .stdout;
    // Lua 5.4.8  Copyright (C) 1994-2025 Lua.org, PUC-Rio
    let version = String::from_utf8_lossy(&stdout)
        .split_whitespace()
        .nth(1)
        .context("Failed to get Lua version")?
        .parse::<Version>()
        .context("Failed to parse Lua version")?;

    let stdout = Cmd::new("luarocks", "get lua executable")
        .arg("config")
        .arg("variables.LUA")
        .check(true)
        .output()
        .await?
        .stdout;

    let executable = PathBuf::from(String::from_utf8_lossy(&stdout).trim());

    Ok(LuaInfo {
        version,
        executable,
    })
}

impl LanguageImpl for Lua {
    async fn install(
        &self,
        hook: Arc<Hook>,
        store: &Store,
        reporter: &HookInstallReporter,
    ) -> Result<InstalledHook> {
        let progress = reporter.on_install_start(&hook);

        let mut info = InstallInfo::new(
            hook.language,
            hook.dependencies().clone(),
            &store.hooks_dir(),
        )?;

        debug!(%hook, target = %info.env_path.display(), "Installing Lua environment");

        // Check lua and luarocks are installed.
        let lua_info = query_lua_info().await.context("Failed to query Lua info")?;

        // Install dependencies for the remote repository.
        if let Some(repo_path) = hook.repo_path() {
            if let Some(rockspec) = Self::get_rockspec_file(repo_path) {
                Self::install_rockspec(&info.env_path, repo_path, &rockspec).await?;
            }
        }

        // Install additional dependencies.
        for dep in &hook.additional_dependencies {
            Self::install_dependency(&info.env_path, dep).await?;
        }

        info.with_toolchain(lua_info.executable)
            .with_language_version(lua_info.version);

        info.persist_env_path();

        reporter.on_install_complete(progress);

        Ok(InstalledHook::Installed {
            hook,
            info: Arc::new(info),
        })
    }

    async fn check_health(&self, info: &InstallInfo) -> Result<()> {
        let current_lua_info = query_lua_info()
            .await
            .context("Failed to query current Lua info")?;

        if current_lua_info.version != info.language_version {
            anyhow::bail!(
                "Lua version mismatch: expected `{}`, found `{}`",
                info.language_version,
                current_lua_info.version
            );
        }

        if current_lua_info.executable != info.toolchain {
            anyhow::bail!(
                "Lua executable mismatch: expected `{}`, found `{}`",
                info.toolchain.display(),
                current_lua_info.executable.display()
            );
        }

        Ok(())
    }

    async fn run(
        &self,
        hook: &InstalledHook,
        filenames: &[&Path],
        _store: &Store,
    ) -> Result<(i32, Vec<u8>)> {
        let env_dir = hook.env_path().expect("Lua must have env path");
        let new_path = prepend_paths(&[&env_dir.join("bin")]).context("Failed to join PATH")?;
        let entry = hook.entry.resolve(Some(&new_path))?;

        let version = &hook
            .install_info()
            .expect("Lua must have install info")
            .language_version;
        // version without patch, e.g. 5.4
        let version = format!("{}.{}", version.major, version.minor);
        let lua_path = Lua::get_lua_path(env_dir, &version);
        let lua_cpath = Lua::get_lua_cpath(env_dir, &version);

        let run = async |batch: &[&Path]| {
            let mut output = Cmd::new(&entry[0], "run lua command")
                .current_dir(hook.work_dir())
                .args(&entry[1..])
                .env(EnvVars::PATH, &new_path)
                .env(EnvVars::LUA_PATH, &lua_path)
                .env(EnvVars::LUA_CPATH, &lua_cpath)
                .args(&hook.args)
                .args(batch)
                .check(false)
                .stdin(Stdio::null())
                .pty_output()
                .await?;

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

        Ok((combined_status, combined_output))
    }
}

impl Lua {
    async fn install_rockspec(env_path: &Path, root_path: &Path, rockspec: &Path) -> Result<()> {
        Cmd::new("luarocks", "luarocks make rockspec")
            .current_dir(root_path)
            .arg("--tree")
            .arg(env_path)
            .arg("make")
            .arg(rockspec)
            .check(true)
            .output()
            .await
            .context("Failed to install dependency with rockspec")?;
        Ok(())
    }

    async fn install_dependency(env_path: &Path, dependency: &str) -> Result<()> {
        Cmd::new("luarocks", "luarocks install dependency")
            .arg("--tree")
            .arg(env_path)
            .arg("install")
            .arg(dependency)
            .check(true)
            .output()
            .await
            .context("Failed to install Lua dependency")?;
        Ok(())
    }

    fn get_rockspec_file(root_path: &Path) -> Option<PathBuf> {
        if let Ok(entries) = std::fs::read_dir(root_path) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|s| s.to_str()) == Some("rockspec") {
                    return Some(path);
                }
            }
        }
        None
    }

    fn get_lua_path(env_dir: &Path, version: &str) -> String {
        let share_dir = env_dir.join("share");
        format!(
            "{};{};;",
            share_dir.join("lua").join(version).join("?.lua").display(),
            share_dir
                .join("lua")
                .join(version)
                .join("?")
                .join("init.lua")
                .display()
        )
    }

    fn get_lua_cpath(env_dir: &Path, version: &str) -> String {
        let lib_dir = env_dir.join("lib");
        let so_ext = if cfg!(windows) { "dll" } else { "so" };
        format!(
            "{};;",
            lib_dir
                .join("lua")
                .join(version)
                .join(format!("?.{so_ext}"))
                .display()
        )
    }
}
