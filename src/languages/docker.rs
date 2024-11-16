use std::borrow::Cow;
use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use assert_cmd::output::{OutputError, OutputOkExt};
use fancy_regex::Regex;
use tokio::process::Command;
use tracing::debug;

use crate::config::Language;
use crate::fs::CWD;
use crate::hook::Hook;
use crate::languages::{LanguageImpl, DEFAULT_VERSION};
use crate::run::run_by_batch;

const PRE_COMMIT_LABEL: &str = "PRE_COMMIT";

#[derive(Debug, Copy, Clone)]
pub struct Docker;

impl Docker {
    fn docker_tag(hook: &Hook) -> Option<String> {
        hook.path()
            .file_name()
            .and_then(OsStr::to_str)
            .map(|s| format!("pre-commit-{:x}", md5::compute(s)))
    }

    async fn build_docker_image(hook: &Hook, pull: bool) -> anyhow::Result<()> {
        let mut cmd = Command::new("docker");

        let cmd = cmd.arg("build").args([
            "--tag",
            &Self::docker_tag(hook).expect("Tag can't generate"),
            "--label",
            PRE_COMMIT_LABEL,
        ]);

        if pull {
            cmd.arg("--pull");
        }

        // This must come last for old versions of docker.
        // see https://github.com/pre-commit/pre-commit/issues/477
        cmd.arg(".");

        debug!(cmd = ?cmd, "docker build_docker_image:");

        cmd.current_dir(hook.path())
            .output()
            .await
            .map_err(OutputError::with_cause)?
            .ok()?;

        Ok(())
    }

    fn is_in_docker() -> bool {
        match fs_err::read_to_string("/proc/self/mountinfo") {
            Ok(mounts) => mounts.contains("docker"),
            Err(_) => false,
        }
    }

    /// It should check [`Self::is_in_docker`] first
    ///
    /// there are no valid algorithm to get container id inner container, see <https://stackoverflow.com/questions/20995351/how-can-i-get-docker-linux-container-information-from-within-the-container-itsel>
    fn get_container_id() -> String {
        // https://github.com/open-telemetry/opentelemetry-java-instrumentation/pull/7167/files
        let regex = Regex::new(r".*/docker/containers/([0-9a-f]{64})/.*").unwrap();
        let v2_group_path = fs_err::read_to_string("/proc/self/mountinfo")
            .expect("Failed to find the container ID");

        let captures = regex.captures(&v2_group_path).unwrap().unwrap();
        let id = captures.get(1).unwrap().as_str();
        id.to_string()
    }

    async fn get_docker_path(path: &Path) -> Cow<'_, str> {
        if !Self::is_in_docker() {
            return path.to_string_lossy();
        };

        let container_id = Self::get_container_id();
        if let Ok(output) = Command::new("docker")
            .args(["inspect", "--format", "'{{json .Mounts}}'", &container_id])
            .output()
            .await
        {
            if let Ok(Value::Array(array)) = serde_json::from_slice(&output.stdout) {
                for item in array {
                    if let Value::Object(map) = item {
                        let src_path = map.get("Source").unwrap().as_str().unwrap();
                        let to_path: &Path =
                            map.get("Destination").unwrap().as_str().unwrap().as_ref();

                        if path.starts_with(to_path) {
                            if let Some(to_path) = to_path.to_str() {
                                return Cow::from(
                                    path.to_string_lossy().replace(to_path, src_path),
                                );
                            }
                        }
                    }
                }
            }
        }

        path.to_string_lossy()
    }

    /// This aim to run as non-root user
    ///
    /// ## Windows:
    ///
    /// no way, see <https://docs.docker.com/desktop/setup/install/windows-permission-requirements/>
    ///
    /// ## Other Unix Platform
    ///
    /// see <https://stackoverflow.com/questions/57951893/how-to-determine-the-effective-user-id-of-a-process-in-rust>
    #[cfg(unix)]
    fn get_docker_user() -> [String; 2] {
        unsafe {
            [
                "-u".to_owned(),
                format!("{}:{}", libc::geteuid(), libc::geteuid()),
            ]
        }
    }

    #[cfg(not(unix))]
    fn get_docker_user() -> [String; 0] {
        []
    }

    fn get_docker_tty(color: bool) -> Option<String> {
        if color {
            Some("--tty".to_owned())
        } else {
            None
        }
    }

    async fn docker_cmd(color: bool) -> Command {
        let mut command = Command::new("docker");
        command.args(["run", "--rm"]);
        if let Some(tty) = Self::get_docker_tty(color) {
            command.arg(&tty);
        }

        command.args(Self::get_docker_user());

        command.args([
            "-v",
            // https://docs.docker.com/engine/reference/commandline/run/#mount-volumes-from-container-volumes-from
            &format!("{}:/src:rw,Z", Self::get_docker_path(&CWD).await),
            "--workdir",
            "/src",
        ]);

        command
    }
}

impl LanguageImpl for Docker {
    fn name(&self) -> Language {
        Language::Docker
    }

    fn default_version(&self) -> &str {
        DEFAULT_VERSION
    }

    fn environment_dir(&self) -> Option<&str> {
        Some("docker")
    }

    async fn install(&self, hook: &Hook) -> anyhow::Result<()> {
        let env = hook.environment_dir().expect("No environment dir found");
        debug!(path = ?hook.path(), env=?env, "docker install:");
        Docker::build_docker_image(hook, true).await?;
        fs_err::create_dir_all(env)?;
        Ok(())
    }

    async fn check_health(&self) -> anyhow::Result<()> {
        todo!()
    }

    async fn run(
        &self,
        hook: &Hook,
        filenames: &[&String],
        env_vars: Arc<HashMap<&'static str, String>>,
    ) -> anyhow::Result<(i32, Vec<u8>)> {
        Docker::build_docker_image(hook, false).await?;

        let docker_tag = Docker::docker_tag(hook).unwrap();

        let cmds = shlex::split(&hook.entry).ok_or(anyhow::anyhow!("Failed to parse entry"))?;

        let cmds = Arc::new(cmds);
        let hook_args = Arc::new(hook.args.clone());

        let run = move |batch: Vec<String>| {
            let cmds = cmds.clone();
            let docker_tag = docker_tag.clone();
            let hook_args = hook_args.clone();
            let env_vars = env_vars.clone();

            async move {
                // docker run [OPTIONS] IMAGE [COMMAND] [ARG...]
                let mut cmd = Docker::docker_cmd(true).await;
                let cmd = cmd
                    .args(["--entrypoint", &cmds[0], &docker_tag])
                    .args(&cmds[1..])
                    .args(hook_args.as_ref())
                    .args(batch)
                    .stderr(std::process::Stdio::inherit())
                    .envs(env_vars.as_ref());

                debug!(cmd = ?cmd, "Docker run batch:");

                let mut output = cmd.output().await?;
                output.stdout.extend(output.stderr);
                let code = output.status.code().unwrap_or(1);
                anyhow::Ok((code, output.stdout))
            }
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
