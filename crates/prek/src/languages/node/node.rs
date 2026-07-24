use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use prek_consts::env_vars::EnvVars;
use prek_consts::prepend_paths;
use semver::Version;
use tracing::debug;
use url::Url;

use crate::cli::reporter::HookInstallReporter;
use crate::cli::run::HookRunReporter;
use crate::hook::InstalledHook;
use crate::hook::{Hook, InstallInfo};
use crate::languages::LanguageBackend;
use crate::languages::node::NodeRequest;
use crate::languages::node::installer::{NodeInstaller, bin_dir, lib_dir, query_node_version};
use crate::languages::node::version::EXTRA_KEY_LTS;
use crate::languages::version::LanguageRequest;
use crate::process::Cmd;
use crate::run::run_by_batch;
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

        let (node_request, allows_download) = match &hook.language_request {
            LanguageRequest::Any { system_only } => (&NodeRequest::Any, !system_only),
            LanguageRequest::Node(node_request) => (node_request, true),
            _ => unreachable!(),
        };
        let node = installer
            .install(store, node_request, allows_download)
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
        let (deps, includes_git_hook_repo) = node_install_dependencies(&hook)?;
        if deps.is_empty() {
            debug!("No dependencies to install");
        } else {
            // Why remote hook repositories are installed as `git+file://` rather than as folders
            // ------------------------------------------------------------------------------------
            //
            // npm delegates package acquisition to `pacote`. The type of the package spec selects
            // a fetcher, and folder and Git specs have importantly different preparation semantics:
            //
            // * `<folder>` (including `<folder>` with `--install-links`) selects `DirFetcher`.
            //   `DirFetcher` runs the source package's `prepare` script and then packs the directory.
            //   It does *not* first run a nested install in that source directory. Although Arborist
            //   has resolved the package's dependency tree, those dependencies have not yet been
            //   reified into `<folder>/node_modules` when `DirFetcher` needs to prepare and pack it.
            //   Consequently, a conventional source package such as
            //
            //       devDependencies: { "typescript": "..." }
            //       scripts:         { "prepare": "tsc" }
            //
            //   fails with `tsc: not found`. `--install-links` only changes whether directory
            //   content is packed instead of linked; it does not add the missing install-before-
            //   prepare step.
            //
            // * `git+file://<repo>` selects `GitFetcher`. It clones the already-pinned local
            //   checkout into npm's temporary cache. In npm 12, when the package needs
            //   preparation, `GitFetcher` runs a nested, non-global install roughly equivalent to:
            //
            //       npm install --force --include=dev --include=peer --include=optional \
            //         --global=false
            //
            //   The nested install makes build-time dependencies available and runs the root
            //   package's `prepare`; `DirFetcher` then packs that prepared temporary clone, and
            //   the outer global install installs the packed result into the hook environment.
            //   This is npm's documented behavior for Git dependencies and matches what package
            //   authors expect when publishing source that must be compiled before use.
            //
            // Besides fixing lifecycle ordering, the temporary Git clone keeps `node_modules` and
            // generated build output out of prek's shared repository cache. The extra local clone
            // and pack are deliberate costs in exchange for correct, isolated package preparation.
            //
            // npm 12 defaults `allow-git` to `none`, so prek must explicitly opt this top-level
            // Git package into fetching. `root` is intentionally narrower than `all`: it permits
            // Git dependencies introduced by this npm command's project root, while transitive
            // Git dependencies remain blocked. Because all specs below share one npm command, a
            // Git URL explicitly supplied through `additional_dependencies` is also a root
            // dependency and is therefore allowed. We intentionally do not enable `allow-remote`,
            // `allow-scripts`, or unrestricted `allow-git=all`; npm's other safety defaults remain
            // in effect.
            //
            // npm < 12 does not need the allow flag and older GitFetcher implementations also
            // lack the explicit `--global=false` on their nested install. That means a build
            // which requires devDependencies during `prepare` is only fixed by this path on npm
            // 12 or newer; it already failed with prek's previous folder installation on older
            // npm.
            //
            // In particular, do not pass `--allow-git=root` to npm 11.9 through 11.12. Those
            // releases have an npm bug, not a different definition of a root dependency. The
            // first manifest fetch used to discover an unnamed CLI Git spec correctly receives
            // `_isRoot=true`, and Arborist creates an edge from the project root. A later
            // manifest fetch and the reify/extract path, however, fail to forward that context
            // to pacote. Pacote defaults a missing `_isRoot` to false and consequently rejects
            // the same root dependency as "non-root" with EALLOWGIT. npm 11 defaults
            // `allow-git` to `all`, which masks the bug unless `root` is explicitly requested.
            // The bug was fixed upstream and backported in npm 11.13:
            //
            // - https://github.com/npm/cli/issues/9189
            // - https://github.com/npm/cli/pull/9206
            //
            // Since npm 11 already defaults to allowing Git, prek omits the flag for all npm 11
            // versions to remain compatible with the affected releases. npm 12 both contains the
            // fix and defaults `allow-git` to `none`, so that is where prek starts passing
            // `--allow-git=root`. Querying npm itself instead of inferring from the Node version
            // also covers custom and independently upgraded npm installations correctly.
            //
            // Relevant npm implementation:
            // - pacote/lib/dir.js (`DirFetcher`)
            // - pacote/lib/git.js (`GitFetcher`, especially `#prepareDir`)
            // - @npmcli/arborist/lib/arborist/build-ideal-tree.js (`allow-git` root checks)

            // `npm` is a script that uses `/usr/bin/env node`, so we need to add the
            // node toolchain directory to PATH so that `npm` can find `node`.
            let node_bin = node.node().parent().expect("Node binary must have parent");
            let new_path = prepend_paths(&[&bin_dir, node_bin]).context("Failed to join PATH")?;
            let npm_cache = store.cache_path(CacheBucket::Npm);

            let mut cmd = Cmd::new(node.npm());
            cmd.arg("install");
            if includes_git_hook_repo && query_npm_version(node.npm(), &new_path).await?.major >= 12
            {
                cmd.arg("--allow-git=root");
            }
            cmd.arg("-g")
                .arg("--no-progress")
                .arg("--no-save")
                .arg("--no-fund")
                .arg("--no-audit")
                .args(&deps)
                .env(EnvVars::PATH, new_path)
                .env(EnvVars::NODE_PATH, &lib_dir);
            apply_npm_config_env(&mut cmd, &info.env_path, &npm_cache);
            cmd.check(true).output().await?;
        }

        info.persist_env_path();

        reporter.on_install_complete(progress);

        Ok(InstalledHook::Installed {
            hook,
            info: Arc::new(info),
        })
    }

    async fn check_health(&self, info: &InstallInfo) -> Result<()> {
        let version = query_node_version(&info.toolchain)
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

    async fn run(
        &self,
        store: &Store,
        hook: &InstalledHook,
        filenames: &[&Path],
        reporter: &HookRunReporter,
    ) -> Result<(i32, Vec<u8>)> {
        let progress = reporter.on_run_start(hook, filenames.len());

        let env_dir = hook.env_path().expect("Node must have env path");
        let node_bin = hook.toolchain_dir().expect("Node binary must have parent");
        let new_path =
            prepend_paths(&[&bin_dir(env_dir), node_bin]).context("Failed to join PATH")?;

        let entry = hook.entry.resolve(Some(&new_path), store)?;
        let npm_cache = store.cache_path(CacheBucket::Npm);

        let run = async |batch: &[&Path]| {
            let mut cmd = Cmd::new(&entry[0]);
            cmd.current_dir(hook.work_dir())
                .args(&entry[1..])
                .env(EnvVars::PATH, &new_path)
                .env(EnvVars::NODE_PATH, lib_dir(env_dir))
                .envs(&hook.env);
            apply_npm_config_env(&mut cmd, env_dir, &npm_cache);
            let output = cmd
                .args(&hook.args)
                .file_args(batch)
                .check(false)
                .stdin(Stdio::null())
                .pty_output_with_sink(reporter.output_sink(progress))
                .await?;

            reporter.on_run_progress(progress, batch.len() as u64);

            anyhow::Ok(output)
        };

        let output = run_by_batch(hook, filenames, entry.argv(), run).await?;

        reporter.on_run_complete(progress);

        Ok(output)
    }
}

/// Build the npm package specs for a Node hook installation.
///
/// A remote hook repo is included only when it is both a Git checkout and an npm
/// package. Mirror-style hook repos without a root `package.json` can still install
/// their declared `additional_dependencies`. Local hooks never install the user's
/// project as a package; they install only their explicit additional dependencies.
fn node_install_dependencies(hook: &Hook) -> Result<(Vec<String>, bool)> {
    let mut deps = Vec::with_capacity(hook.additional_dependencies.len() + 1);
    let mut includes_git_hook_repo = false;

    if let Some(repo_path) = hook.repo_path()
        && repo_path.join(".git").exists()
        && repo_path.join("package.json").exists()
    {
        let file_url = Url::from_file_path(repo_path).map_err(|()| {
            anyhow!(
                "Failed to convert Node hook repository path to a file URL: {}",
                repo_path.display()
            )
        })?;
        deps.push(format!("git+{file_url}"));
        includes_git_hook_repo = true;
    }

    deps.extend(hook.additional_dependencies.iter().cloned());
    Ok((deps, includes_git_hook_repo))
}

async fn query_npm_version(npm: &Path, path: &std::ffi::OsStr) -> Result<Version> {
    let output = Cmd::new(npm)
        .arg("--version")
        .env(EnvVars::PATH, path)
        .check(true)
        .output()
        .await?;
    Version::parse(String::from_utf8_lossy(&output.stdout).trim())
        .context("Failed to parse npm version")
}

fn apply_npm_config_env(cmd: &mut Cmd, prefix: &Path, cache: &Path) {
    for key in NPM_CONFIG_ENVS_TO_REMOVE {
        cmd.env_remove(key);
    }
    cmd.env(NPM_CONFIG_PREFIX_ENV, prefix);
    cmd.env(NPM_CONFIG_CACHE_ENV, cache);
}
