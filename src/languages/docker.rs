use std::borrow::Cow;
use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs;
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

    async fn build_docker_image(hook: &Hook, pull: bool) -> Result<()> {
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

    /// see <https://stackoverflow.com/questions/23513045/how-to-check-if-a-process-is-running-inside-docker-container>
    fn is_in_docker() -> bool {
        if fs::metadata("/.dockerenv").is_ok() || fs::metadata("/run/.containerenv").is_ok() {
            return true;
        }
        false
    }

    /// It should check [`Self::is_in_docker`] first, but like [Codespaces](https://github.com/features/codespaces) also run inner docker.
    ///
    /// There are no valid algorithm to get container id inner container, see
    /// <https://stackoverflow.com/questions/20995351/how-can-i-get-docker-linux-container-information-from-within-the-container-itsel>
    fn get_container_id() -> Option<String> {
        // copy from https://github.com/open-telemetry/opentelemetry-java-instrumentation/pull/7167/files
        if let Ok(regex) = Regex::new(r".*/docker/containers/([0-9a-f]{64})/.*") {
            if let Ok(v2_group_path) = fs_err::read_to_string("/proc/self/mountinfo") {
                if let Ok(Some(captures)) = regex.captures(&v2_group_path) {
                    return captures.get(1).map(|m| m.as_str().to_string());
                }
            }
        }

        None
    }

    async fn get_docker_path(path: &Path) -> Result<Cow<'_, str>> {
        if !Self::is_in_docker() {
            return Ok(path.to_string_lossy());
        };

        let Some(container_id) = Self::get_container_id() else {
            return Ok(path.to_string_lossy());
        };

        debug!(%container_id, "Docker get_docker_path:");

        if let Ok(output) = Command::new("docker")
            .args(["inspect", "--format", "'{{json .Mounts}}'", &container_id])
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
            debug!(?output, "Docker get_docker_path:");

            // using test env Dockerfile return around `'` and end with `\n`
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stdout = stdout.trim().trim_matches('\'');
            let mounts: Vec<Mount> = serde_json::from_str(stdout)?;

            debug!(?mounts, ?path, "Docker get_docker_path:");

            for mount in mounts {
                if path.starts_with(&mount.destination) {
                    let mut res = path
                        .to_string_lossy()
                        .replace(&mount.destination, &mount.source);
                    if res.contains('\\') {
                        // that means runner on the win
                        res = res.replace('/', "\\");
                    }
                    return Ok(Cow::Owned(res));
                }
            }
        }

        Ok(path.to_string_lossy())
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

    async fn docker_cmd(color: bool) -> Result<Command> {
        let mut command = Command::new("docker");
        command.args(["run", "--rm"]);
        if let Some(tty) = Self::get_docker_tty(color) {
            command.arg(&tty);
        }

        command.args(Self::get_docker_user()).args([
            "-v",
            // https://docs.docker.com/engine/reference/commandline/run/#mount-volumes-from-container-volumes-from
            &format!("{}:/src:rw,Z", Self::get_docker_path(&CWD).await?),
            "--workdir",
            "/src",
        ]);

        Ok(command)
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
        None
    }

    async fn install(&self, hook: &Hook) -> Result<()> {
        let env = hook.environment_dir().expect("No environment dir found");
        debug!(path = ?hook.path(), env=?env, "docker install:");
        Docker::build_docker_image(hook, true).await?;
        fs_err::create_dir_all(env)?;
        Ok(())
    }

    async fn check_health(&self) -> Result<()> {
        todo!()
    }

    async fn run(
        &self,
        hook: &Hook,
        filenames: &[&String],
        env_vars: Arc<HashMap<&'static str, String>>,
    ) -> Result<(i32, Vec<u8>)> {
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
                let mut cmd = Docker::docker_cmd(true).await?;
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

#[cfg(test)]
mod tests {
    use super::Docker;
    use std::env;
    use std::path::Path;
    use tracing::debug;
    use tracing_test::traced_test;

    // This test should run by docker build by [Dockerfile](../../.github/fixture/Dockerfile)
    #[test]
    #[ignore]
    #[traced_test]
    fn test_get_docker_path() {
        assert!(Docker::is_in_docker());
        let env_path = env::var("OUTSIDE_PATH").unwrap();

        debug!(%env_path, "test_get_docker_path");

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("Failed to create tokio runtime");

        let path = Path::new("./outside/test/uv-pre-commit-config.yaml")
            .canonicalize()
            .unwrap();

        let result = runtime.block_on(Docker::get_docker_path(&path)).unwrap();

        assert_eq!(result, env_path);
    }
}
