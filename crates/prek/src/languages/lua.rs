use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use prek_consts::env_vars::EnvVars;
use prek_consts::prepend_paths;
use semver::Version;
use tracing::debug;

use crate::cli::reporter::HookInstallReporter;
use crate::hook::{Hook, InstallInfo, InstalledHook};
use crate::languages::{ExecutionEnvironment, LanguageBackend};
use crate::process::Cmd;
use crate::store::Store;

#[derive(Debug, Copy, Clone)]
pub(crate) struct Lua;

pub(crate) struct LuaInfo {
    pub(crate) version: Version,
    pub(crate) executable: std::path::PathBuf,
}

pub(crate) async fn query_lua_info() -> Result<LuaInfo> {
    let stdout = Cmd::new("lua").arg("-v").check(true).output().await?.stdout;
    // Lua 5.4.8  Copyright (C) 1994-2025 Lua.org, PUC-Rio
    let version = String::from_utf8_lossy(&stdout)
        .split_whitespace()
        .nth(1)
        .context("Failed to get Lua version")?
        .parse::<Version>()
        .context("Failed to parse Lua version")?;

    let stdout = Cmd::new("luarocks")
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

#[async_trait::async_trait(?Send)]
impl LanguageBackend for Lua {
    async fn install(
        &self,
        store: &Store,
        hook: Arc<Hook>,
        install_cwd: &Path,
        reporter: &HookInstallReporter,
    ) -> Result<InstalledHook> {
        let progress = reporter.on_install_start(&hook);

        let mut info = InstallInfo::new(&hook, &store.hooks_dir())?;

        debug!(%hook, target = %info.env_path.display(), "Installing Lua environment");

        // Check lua and luarocks are installed.
        let lua_info = query_lua_info().await.context("Failed to query Lua info")?;

        // Install dependencies for the remote repository.
        if let Some(repo_path) = hook.repo_path() {
            if let Some(rockspec) = Self::get_rockspec_file(repo_path) {
                Self::install_rockspec(&info.env_path, install_cwd, &rockspec).await?;
            }
        }

        // Install additional dependencies.
        for dep in &hook.additional_dependencies {
            Self::install_dependency(&info.env_path, install_cwd, dep).await?;
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

    fn execution_environment(
        &self,
        _store: &Store,
        hook: &InstalledHook,
    ) -> Result<ExecutionEnvironment> {
        let env_dir = hook.env_path().expect("Lua must have env path");
        let new_path = prepend_paths(&[&env_dir.join("bin")]).context("Failed to join PATH")?;
        let version = &hook
            .install_info()
            .expect("Lua must have install info")
            .language_version;
        let version = format!("{}.{}", version.major, version.minor);

        let mut environment = ExecutionEnvironment::new();
        environment
            .set_path(&new_path)
            .env(EnvVars::LUA_PATH, Lua::get_lua_path(env_dir, &version))
            .env(EnvVars::LUA_CPATH, Lua::get_lua_cpath(env_dir, &version));
        Ok(environment)
    }
}

impl Lua {
    async fn install_rockspec(env_path: &Path, install_cwd: &Path, rockspec: &Path) -> Result<()> {
        Cmd::new("luarocks")
            .current_dir(install_cwd)
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

    async fn install_dependency(
        env_path: &Path,
        install_cwd: &Path,
        dependency: &str,
    ) -> Result<()> {
        Cmd::new("luarocks")
            .current_dir(install_cwd)
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
        if let Ok(entries) = fs_err::read_dir(root_path) {
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
