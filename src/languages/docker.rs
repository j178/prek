use std::borrow::Cow;
use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, LazyLock};

use anyhow::{Context, Result};
use fancy_regex::Regex;
use tracing::trace;

use crate::cli::reporter::HookInstallReporter;
use crate::fs::CWD;
use crate::hook::{Hook, InstallInfo, InstalledHook};
use crate::languages::LanguageImpl;
use crate::process::Cmd;
use crate::run::{USE_COLOR, run_by_batch};
use crate::store::Store;

#[derive(Debug, Copy, Clone)]
pub(crate) struct Docker;

#[derive(serde::Deserialize, Debug)]
struct Mount {
    #[serde(rename = "Source")]
    source: String,
    #[serde(rename = "Destination")]
    destination: String,
}

#[derive(Debug, thiserror::Error)]
enum Error {
    #[error("Failed to parse docker inspect output: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("Failed to run `docker inspect`: {0}")]
    Process(#[from] std::io::Error),
}

static CONTAINER_MOUNTS: LazyLock<Result<Vec<Mount>, Error>> = LazyLock::new(|| {
    if !Docker::is_in_docker() {
        trace!("Not in Docker");
        return Ok(vec![]);
    }

    let Ok(container_id) = Docker::current_container_id() else {
        return Ok(vec![]);
    };

    trace!(?container_id, "Get docker container id");

    let output = std::process::Command::new("docker")
        .arg("inspect")
        .arg("--format")
        .arg("'{{json .Mounts}}'")
        .arg(&container_id)
        .output()?
        .stdout;
    let stdout = String::from_utf8_lossy(&output);
    let stdout = stdout.trim().trim_matches('\'');
    let mounts: Vec<Mount> = serde_json::from_str(stdout)?;
    trace!(?mounts, "Get docker mounts");

    Ok(mounts)
});

impl Docker {
    fn docker_tag(hook: &InstalledHook) -> String {
        let InstalledHook::Installed { info, .. } = hook else {
            panic!("Docker tag can only be generated for installed hooks");
        };
        let mut hasher = DefaultHasher::new();
        info.hash(&mut hasher);
        let digest = hex::encode(hasher.finish().to_le_bytes());
        format!("prek-{digest}")
    }

    async fn build_docker_image(hook: &InstalledHook, pull: bool) -> Result<String> {
        let Some(src) = hook.repo_path() else {
            anyhow::bail!("Language `docker` cannot work with `local` repository");
        };

        let tag = Self::docker_tag(hook);
        let mut cmd = Cmd::new("docker", "build docker image");
        let cmd = cmd
            .arg("build")
            .arg("--tag")
            .arg(&tag)
            .arg("--label")
            .arg("org.opencontainers.image.vendor=prek")
            .arg("--label")
            .arg(format!("org.opencontainers.image.source={}", hook.repo()))
            .arg("--label")
            .arg(format!("prek.hook.id={}", hook.id))
            .arg("--label")
            .arg("prek.managed=true");

        // Always attempt to pull all referenced images.
        if pull {
            cmd.arg("--pull");
        }

        // This must come last for old versions of docker.
        // see https://github.com/pre-commit/pre-commit/issues/477
        cmd.arg(".");

        cmd.current_dir(src).check(true).output().await?;

        Ok(tag)
    }

    /// see <https://stackoverflow.com/questions/23513045/how-to-check-if-a-process-is-running-inside-docker-container>
    fn is_in_docker() -> bool {
        if fs::metadata("/.dockerenv").is_ok() || fs::metadata("/run/.containerenv").is_ok() {
            return true;
        }
        false
    }

    /// Get container id the process is running in.
    ///
    /// There are no reliable way to get the container id inside container, see
    /// <https://stackoverflow.com/questions/20995351/how-can-i-get-docker-linux-container-information-from-within-the-container-itsel>
    fn current_container_id() -> Result<String> {
        // Adapted from https://github.com/open-telemetry/opentelemetry-java-instrumentation/pull/7167/files
        let regex = Regex::new(r".*/docker/containers/([0-9a-f]{64})/.*").expect("invalid regex");
        let cgroup_path = fs::read_to_string("/proc/self/cgroup")?;
        let Some(captures) = regex.captures(&cgroup_path)? else {
            anyhow::bail!("Failed to get container id: no match found");
        };
        let Some(id) = captures.get(1).map(|m| m.as_str().to_string()) else {
            anyhow::bail!("Failed to get container id: no capture found");
        };
        Ok(id)
    }

    /// Get the path of the current directory in the host.
    fn get_docker_path(path: &str) -> Result<Cow<'_, str>> {
        let mounts = CONTAINER_MOUNTS.as_ref()?;

        for mount in mounts {
            if path.starts_with(&mount.destination) {
                let mut path = path.replace(&mount.destination, &mount.source);
                if path.contains('\\') {
                    // Replace `/` with `\` on Windows
                    path = path.replace('/', "\\");
                }
                return Ok(Cow::Owned(path));
            }
        }

        Ok(Cow::Borrowed(path))
    }

    pub(crate) fn docker_run_cmd() -> Result<Cmd> {
        let mut command = Cmd::new("docker", "run container");
        command.arg("run").arg("--rm");

        if *USE_COLOR {
            command.arg("--tty");
        }

        // Run as a non-root user
        #[cfg(unix)]
        {
            command.arg("--user");
            command.arg(format!("{}:{}", unsafe { libc::geteuid() }, unsafe {
                libc::getegid()
            }));
        }

        // TODO: fix in workspace mode
        let cwd = &CWD.to_string_lossy();
        let work_dir = Self::get_docker_path(cwd)?;
        command
            .arg("-v")
            // https://docs.docker.com/engine/reference/commandline/run/#mount-volumes-from-container-volumes-from
            // The `Z` option tells Docker to label the content with a private
            // unshared label. Only the current container can use a private volume.
            .arg(format!("{work_dir}:/src:rw,Z",))
            .arg("--workdir")
            .arg("/src");

        Ok(command)
    }
}

impl LanguageImpl for Docker {
    async fn install(
        &self,
        hook: Arc<Hook>,
        store: &Store,
        reporter: &HookInstallReporter,
    ) -> Result<InstalledHook> {
        let progress = reporter.on_install_start(&hook);

        let info = InstallInfo::new(
            hook.language,
            hook.dependencies().clone(),
            &store.hooks_dir(),
        );
        let installed_hook = InstalledHook::Installed {
            hook,
            info: Arc::new(info),
        };

        Docker::build_docker_image(&installed_hook, true)
            .await
            .context("Failed to build docker image")?;
        let env = installed_hook
            .env_path()
            .expect("Docker must have env path");

        fs_err::tokio::create_dir_all(env)
            .await
            .context("Failed to create docker env dir")?;

        reporter.on_install_complete(progress);

        Ok(installed_hook)
    }

    async fn check_health(&self) -> Result<()> {
        todo!()
    }

    async fn run(
        &self,
        hook: &InstalledHook,
        filenames: &[&String],
        _store: &Store,
    ) -> Result<(i32, Vec<u8>)> {
        let docker_tag = Docker::build_docker_image(hook, false)
            .await
            .context("Failed to build docker image")?;
        let entry = hook.entry.resolve(None)?;

        let run = async move |batch: &[&String]| {
            // docker run [OPTIONS] IMAGE [COMMAND] [ARG...]
            let mut cmd = Docker::docker_run_cmd()?;
            let mut output = cmd
                .arg("--entrypoint")
                .arg(&entry[0])
                .arg(&docker_tag)
                .args(&entry[1..])
                .args(&hook.args)
                .args(batch)
                .check(false)
                .output()
                .await?;

            output.stdout.extend(output.stderr);
            let code = output.status.code().unwrap_or(1);
            anyhow::Ok((code, output.stdout))
        };

        let results = run_by_batch(hook, filenames, run).await?;

        // Collect results
        let mut combined_status = 0;
        let mut combined_output = Vec::new();

        for (code, output) in results {
            combined_status |= code;
            combined_output.extend(output);
        }

        Ok((combined_status, combined_output))
    }
}
