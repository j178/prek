use std::borrow::Cow;
use std::collections::HashMap;
use std::fs;
use std::hash::{Hash, Hasher};

use anstream::ColorChoice;
use anyhow::Result;
use fancy_regex::Regex;
use seahash::SeaHasher;
use tracing::trace;

use crate::fs::CWD;
use crate::hook::{Hook, ResolvedHook};
use crate::languages::LanguageImpl;
use crate::process::Cmd;
use crate::run::run_by_batch;
use crate::store::Store;

const PRE_COMMIT_LABEL: &str = "PRE_COMMIT";

#[derive(Debug, Copy, Clone)]
pub struct Docker;

impl Docker {
    fn docker_tag(hook: &ResolvedHook) -> String {
        let mut hasher = SeaHasher::new();
        hook.hash(&mut hasher);
        let digest = crate::store::to_hex(hasher.finish());
        format!("prefligit-{digest}")
    }

    async fn build_docker_image(hook: &ResolvedHook, pull: bool) -> Result<()> {
        let Some(src) = hook.repo_path() else {
            anyhow::bail!("Language `docker` cannot work with `local` repository");
        };

        let mut cmd = Cmd::new("docker", "build docker image");

        let cmd = cmd
            .arg("build")
            .arg("--tag")
            .arg(Self::docker_tag(hook))
            .arg("--label")
            .arg(PRE_COMMIT_LABEL);

        // Always attempt to pull all referenced images.
        if pull {
            cmd.arg("--pull");
        }

        // This must come last for old versions of docker.
        // see https://github.com/pre-commit/pre-commit/issues/477
        cmd.arg(".");

        cmd.current_dir(src).check(true).output().await?;

        Ok(())
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
    async fn get_docker_path(path: &str) -> Result<Cow<'_, str>> {
        if !Self::is_in_docker() {
            return Ok(Cow::Borrowed(path));
        }

        let Ok(container_id) = Self::current_container_id() else {
            return Ok(Cow::Borrowed(path));
        };

        trace!(?container_id, "Get container id");

        if let Ok(output) = Cmd::new("docker", "inspect container")
            .arg("inspect")
            .arg("--format")
            .arg("'{{json .Mounts}}'")
            .arg(&container_id)
            .check(true)
            .output()
            .await
        {
            #[derive(serde::Deserialize, Debug)]
            struct Mount {
                #[serde(rename = "Source")]
                source: String,
                #[serde(rename = "Destination")]
                destination: String,
            }

            let stdout = String::from_utf8_lossy(&output.stdout);
            let stdout = stdout.trim().trim_matches('\'');
            let mounts: Vec<Mount> = serde_json::from_str(stdout)?;

            trace!(?mounts, "Get docker mounts");

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
        }

        Ok(Cow::Borrowed(path))
    }

    pub(crate) async fn docker_run_cmd() -> Result<Cmd> {
        let mut command = Cmd::new("docker", "run container");
        command.arg("run").arg("--rm");

        match ColorChoice::global() {
            ColorChoice::Always | ColorChoice::AlwaysAnsi => {
                command.arg("--tty");
            }
            _ => {}
        }

        // Run as a non-root user
        #[cfg(unix)]
        {
            command.arg("--user");
            command.arg(format!("{}:{}", unsafe { libc::geteuid() }, unsafe {
                libc::getegid()
            }));
        }

        command
            .arg("-v")
            // https://docs.docker.com/engine/reference/commandline/run/#mount-volumes-from-container-volumes-from
            .arg(format!(
                "{}:/src:ro,Z",
                Self::get_docker_path(&CWD.to_string_lossy()).await?
            ))
            .arg("--workdir")
            .arg("/src");

        Ok(command)
    }
}

impl LanguageImpl for Docker {
    fn supports_dependency(&self) -> bool {
        true
    }

    async fn resolve(&self, hook: &Hook, _store: &Store) -> Result<ResolvedHook> {
        Ok(ResolvedHook::NoNeedInstall(hook.clone()))
    }

    async fn install(&self, hook: &ResolvedHook, _store: &Store) -> Result<()> {
        let env = hook.env_path().expect("Docker must have env path");

        // TODO: check unsupported language version
        if !hook.additional_dependencies.is_empty() {
            anyhow::bail!("Docker does not support additional dependencies");
        }

        Docker::build_docker_image(hook, true).await?;
        fs_err::tokio::create_dir_all(env).await?;
        Ok(())
    }

    async fn check_health(&self) -> Result<()> {
        todo!()
    }

    async fn run(
        &self,
        hook: &ResolvedHook,
        filenames: &[&String],
        env_vars: &HashMap<&'static str, String>,
        _store: &Store,
    ) -> Result<(i32, Vec<u8>)> {
        Docker::build_docker_image(hook, false).await?;

        let docker_tag = Docker::docker_tag(hook);

        let cmds = shlex::split(&hook.entry).ok_or(anyhow::anyhow!("Failed to parse entry"))?;

        let run = async move |batch: Vec<String>| {
            // docker run [OPTIONS] IMAGE [COMMAND] [ARG...]
            let mut cmd = Docker::docker_run_cmd().await?;
            let cmd = cmd
                .arg("--entrypoint")
                .arg(&cmds[0])
                .arg(&docker_tag)
                .args(&cmds[1..])
                .args(&hook.args)
                .args(batch)
                .check(false)
                .envs(env_vars);

            let mut output = cmd.output().await?;
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
