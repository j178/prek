use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::str;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use prek_consts::env_vars::EnvVars;
use prek_consts::prepend_paths;
use semver::Version;
use tracing::debug;
use url::Url;

use crate::cli::reporter::HookInstallReporter;
use crate::git::GitCommandExt;
use crate::hook::InstalledHook;
use crate::hook::{Hook, InstallInfo};
use crate::languages::node::NodeRequest;
use crate::languages::node::installer::{
    NodeInstaller, bin_dir, lib_dir, query_node_version_cached,
};
use crate::languages::node::version::EXTRA_KEY_LTS;
use crate::languages::{ExecutionEnvironment, LanguageBackend};
use crate::process::Cmd;
use crate::store::{CacheBucket, Store, ToolBucket};

#[derive(Debug, Copy, Clone)]
pub(crate) struct Node;

const NPM_CONFIG_PREFIX_ENV: &str = "npm_config_prefix";
const NPM_CONFIG_CACHE_ENV: &str = "npm_config_cache";
// npm exports `global_prefix` and `local_prefix` as lowercase child-process
// state, not npmrc config sources. It accepts either case when reading env, so
// clear both forms to keep parent npm/npx context out of the hook env while
// preserving user/global npmrc paths for auth.
const NPM_CONFIG_ENVS_TO_REMOVE: &[&str] = &[
    "NPM_CONFIG_PREFIX",
    "npm_config_prefix",
    "NPM_CONFIG_GLOBAL_PREFIX",
    "npm_config_global_prefix",
    "NPM_CONFIG_LOCAL_PREFIX",
    "npm_config_local_prefix",
    "NPM_CONFIG_CACHE",
    "npm_config_cache",
];

#[async_trait::async_trait(?Send)]
impl LanguageBackend for Node {
    async fn install(
        &self,
        store: &Store,
        hook: Arc<Hook>,
        install_cwd: &Path,
        reporter: &HookInstallReporter,
    ) -> Result<InstalledHook> {
        let progress = reporter.on_install_start(&hook);

        // 1. Install node
        //   1) Find from `$PREK_HOME/tools/node`
        //   2) Find from system
        //   3) Download from remote
        // 2. Create env
        // 3. Install dependencies

        // 1. Install node
        let node_dir = store.tools_path(ToolBucket::Node);
        let installer = NodeInstaller::new(node_dir);

        let node_request: &NodeRequest = hook.language_request.version();
        let node = installer
            .install(
                store,
                node_request,
                hook.language_request.toolchain_policy(),
            )
            .await
            .context("Failed to install node")?;

        let mut info = InstallInfo::new(&hook, &store.hooks_dir())?;

        let lts = serde_json::to_string(&node.version().lts).context("Failed to serialize LTS")?;
        info.with_toolchain(node.node().to_path_buf());
        info.with_language_version(node.version().version.clone());
        info.with_extra(EXTRA_KEY_LTS, &lts);

        // 2. Create env
        let bin_dir = bin_dir(&info.env_path);
        let lib_dir = lib_dir(&info.env_path);
        fs_err::tokio::create_dir_all(&bin_dir).await?;
        fs_err::tokio::create_dir_all(&lib_dir).await?;

        // 3. Install dependencies
        let hook_repo = node_hook_repo_spec(&hook)?;
        if hook_repo.is_none() && hook.additional_dependencies.is_empty() {
            debug!("No dependencies to install");
        } else {
            // `npm` is a script that uses `/usr/bin/env node`, so we need to add the
            // node toolchain directory to PATH so that `npm` can find `node`.
            let node_bin = node.node().parent().expect("Node binary must have parent");
            let new_path = prepend_paths(&[&bin_dir, node_bin]).context("Failed to join PATH")?;
            let npm_cache = store.cache_path(CacheBucket::Npm);
            Npm {
                executable: node.npm(),
                cwd: install_cwd,
                path: &new_path,
                node_path: &lib_dir,
                prefix: &info.env_path,
                cache: &npm_cache,
            }
            .install_dependencies(hook_repo.as_deref(), &hook.additional_dependencies)
            .await?;
        }

        info.persist_env_path();

        reporter.on_install_complete(progress);

        Ok(InstalledHook::Installed {
            hook,
            info: Arc::new(info),
        })
    }

    async fn check_health(&self, info: &InstallInfo) -> Result<()> {
        let version = query_node_version_cached(&info.toolchain)
            .await
            .context("Failed to query node version")?;

        if version.version != info.language_version {
            anyhow::bail!(
                "Node version mismatch: expected {}, found {}",
                info.language_version,
                version.version
            );
        }

        Ok(())
    }

    fn execution_environment(
        &self,
        store: &Store,
        hook: &InstalledHook,
    ) -> Result<ExecutionEnvironment> {
        let env_dir = hook.env_path().expect("Node must have env path");
        let node_bin = hook.toolchain_dir().expect("Node binary must have parent");
        let new_path =
            prepend_paths(&[&bin_dir(env_dir), node_bin]).context("Failed to join PATH")?;
        let npm_cache = store.cache_path(CacheBucket::Npm);

        let mut environment = ExecutionEnvironment::new();
        environment
            .set_path(&new_path)
            .env(EnvVars::NODE_PATH, lib_dir(env_dir));
        for key in NPM_CONFIG_ENVS_TO_REMOVE {
            environment.env_remove(key);
        }
        environment
            .env(NPM_CONFIG_PREFIX_ENV, env_dir)
            .env(NPM_CONFIG_CACHE_ENV, &npm_cache);
        Ok(environment)
    }
}

fn node_hook_repo_spec(hook: &Hook) -> Result<Option<String>> {
    hook.repo_path()
        .map(|repo_path| {
            let file_url = Url::from_file_path(repo_path).map_err(|()| {
                anyhow!(
                    "Failed to convert Node hook repository path to a file URL: {}",
                    repo_path.display()
                )
            })?;
            Ok(format!("git+{file_url}"))
        })
        .transpose()
}

struct PackedNodeHook {
    _temp_dir: tempfile::TempDir,
    archive: PathBuf,
}

enum HookInstall<'a> {
    None,
    Git(&'a str),
    Tarball(PackedNodeHook),
}

struct Npm<'a> {
    executable: &'a Path,
    cwd: &'a Path,
    path: &'a OsStr,
    node_path: &'a Path,
    prefix: &'a Path,
    cache: &'a Path,
}

impl Npm<'_> {
    // Why remote hooks use `git+file://` and two installation paths
    // ----------------------------------------------------------------
    //
    // npm delegates package acquisition to `pacote`. The package spec selects a fetcher, and
    // folder and Git specs have importantly different preparation semantics:
    //
    // * `<folder>` (including `<folder>` with `--install-links`) selects `DirFetcher`.
    //   `DirFetcher` runs the source package's `prepare` script and then packs the directory.
    //   It does *not* first run a nested install in that source directory. Although Arborist has
    //   resolved the package's dependency tree, those dependencies have not yet been reified into
    //   `<folder>/node_modules` when `DirFetcher` needs to prepare and pack it. Consequently, a
    //   conventional source package such as
    //
    //       devDependencies: { "typescript": "..." }
    //       scripts:         { "prepare": "tsc" }
    //
    //   fails with `tsc: not found`. `--install-links` only changes whether directory content is
    //   packed instead of linked; it does not add the missing install-before-prepare step.
    //
    // * `git+file://<repo>` selects `GitFetcher`. It clones the already-pinned local checkout into
    //   npm's temporary cache. When the package needs preparation, `GitFetcher` runs a nested,
    //   non-global install roughly equivalent to:
    //
    //       npm install --force --include=dev --include=peer --include=optional \
    //         --global=false
    //
    //   The nested install makes build-time dependencies available and runs the root package's
    //   `prepare`; `DirFetcher` then packs that prepared temporary clone.
    //
    //   On npm < 12, prek invokes this path with `npm pack`, then installs the resulting tarball
    //   globally. npm 11's global Git reifier can otherwise remove the package root while its
    //   child dependencies are still being extracted, causing ENOENT failures. Separating Git
    //   preparation from the global install avoids that race.
    //
    //   npm 12 keeps the direct global Git install. Its reifier does not have the npm 11 race,
    //   while `npm pack` loses the root classification on a follow-up Git manifest fetch and
    //   rejects it under `allow-git=root`. Keeping the direct path avoids broadening that setting
    //   to `allow-git=all`.
    //
    // Besides fixing lifecycle ordering, the temporary Git clone keeps `node_modules` and
    // generated build output out of prek's shared repository cache. The extra local clone and
    // pack are deliberate costs in exchange for correct, isolated package preparation.
    //
    // npm 12 defaults `allow-git` to `none`, so prek must explicitly opt the hook repo into
    // fetching. `root` is intentionally narrower than `all`: it permits the hook and Git URLs
    // explicitly supplied through `additional_dependencies`, while transitive Git dependencies
    // remain blocked. We intentionally do not enable `allow-remote`, `allow-scripts`, or
    // unrestricted `allow-git=all`; npm's other safety defaults remain in effect.
    //
    // In particular, do not pass `--allow-git=root` to npm 11.9 through 11.12. Those releases
    // have an npm bug, not a different definition of a root dependency. The first manifest fetch
    // used to discover an unnamed CLI Git spec correctly receives `_isRoot=true`, and Arborist
    // creates an edge from the project root. A later manifest fetch and the reify/extract path,
    // however, fail to forward that context to pacote. Pacote defaults a missing `_isRoot` to
    // false and consequently rejects the same root dependency as "non-root" with EALLOWGIT. npm
    // 11 defaults `allow-git` to `all`, which masks the bug unless `root` is explicitly requested.
    // The bug was fixed upstream and backported in npm 11.13:
    //
    // - https://github.com/npm/cli/issues/9189
    // - https://github.com/npm/cli/pull/9206
    //
    // Since npm 11 already defaults to allowing Git, prek omits the flag for all npm 11 versions
    // to remain compatible with the affected releases. npm 12 both contains the fix and defaults
    // `allow-git` to `none`, so that is where prek starts passing `--allow-git=root`. Querying npm
    // itself instead of inferring from the Node version also covers custom and independently
    // upgraded npm installations correctly.
    //
    // Relevant npm implementation:
    // - pacote/lib/dir.js (`DirFetcher`)
    // - pacote/lib/git.js (`GitFetcher`, especially `#prepareDir`)
    // - @npmcli/arborist/lib/arborist/build-ideal-tree.js (`allow-git` root checks)
    async fn install_dependencies(
        &self,
        hook_repo: Option<&str>,
        additional_dependencies: &[String],
    ) -> Result<()> {
        let hook_install = match hook_repo {
            Some(hook_repo) if self.version().await?.major >= 12 => HookInstall::Git(hook_repo),
            Some(hook_repo) => HookInstall::Tarball(self.pack_hook(hook_repo).await?),
            None => HookInstall::None,
        };

        let mut cmd = self.command();
        cmd.arg("install")
            .arg("-g")
            .arg("--no-progress")
            .arg("--no-save")
            .arg("--no-fund")
            .arg("--no-audit");
        match &hook_install {
            HookInstall::None => {}
            HookInstall::Git(hook_repo) => {
                cmd.arg("--allow-git=root").arg(hook_repo);
            }
            HookInstall::Tarball(packed_hook) => {
                cmd.arg(&packed_hook.archive);
            }
        }
        cmd.args(additional_dependencies);
        cmd.check(true).output().await?;
        Ok(())
    }

    async fn pack_hook(&self, hook_repo: &str) -> Result<PackedNodeHook> {
        let temp_dir = tempfile::tempdir().context("Failed to create npm pack directory")?;

        let mut cmd = self.command();
        cmd.arg("pack")
            .arg("--global=false")
            .arg("--pack-destination")
            .arg(temp_dir.path())
            .arg(hook_repo);
        cmd.check(true)
            .output()
            .await
            .context("Failed to pack Node hook repository")?;

        let mut archives = fs_err::read_dir(temp_dir.path())
            .context("Failed to read npm pack directory")?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<std::io::Result<Vec<_>>>()?;
        archives.retain(|path| path.extension().is_some_and(|extension| extension == "tgz"));
        let archive = match archives.as_slice() {
            [archive] => archive.clone(),
            _ => {
                return Err(anyhow!(
                    "npm pack produced {} package archives; expected exactly one",
                    archives.len()
                ));
            }
        };

        Ok(PackedNodeHook {
            _temp_dir: temp_dir,
            archive,
        })
    }

    async fn version(&self) -> Result<Version> {
        let output = Cmd::new(self.executable)
            .current_dir(self.cwd)
            .arg("--version")
            .env(EnvVars::PATH, self.path)
            .check(true)
            .output()
            .await?;
        Version::parse(str::from_utf8(&output.stdout)?.trim())
            .context("Failed to parse npm version")
    }

    fn command(&self) -> Cmd {
        let mut cmd = Cmd::new(self.executable);
        cmd.current_dir(self.cwd)
            .sanitize_git_repo_env()
            .env(EnvVars::PATH, self.path)
            .env(EnvVars::NODE_PATH, self.node_path);
        for key in NPM_CONFIG_ENVS_TO_REMOVE {
            cmd.env_remove(key);
        }
        cmd.env(NPM_CONFIG_PREFIX_ENV, self.prefix);
        cmd.env(NPM_CONFIG_CACHE_ENV, self.cache);
        cmd
    }
}
