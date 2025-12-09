use std::borrow::Cow;
use std::collections::BTreeSet;
use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::str::FromStr;
use std::sync::{Arc, LazyLock};

use anyhow::{Context, Result};
use lazy_regex::regex;
use prek_consts::env_vars::EnvVars;
use tracing::{debug, trace, warn};

use crate::cli::reporter::HookInstallReporter;
use crate::hook::{Hook, InstallInfo, InstalledHook};
use crate::languages::LanguageImpl;
use crate::process::Cmd;
use crate::run::{USE_COLOR, run_by_batch};
use crate::store::Store;

#[derive(Debug, Copy, Clone)]
pub(crate) struct Docker;

#[derive(Debug, thiserror::Error)]
enum Error {
    #[error("Failed to parse docker inspect output: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("Failed to run `docker inspect`: {0}")]
    Process(#[from] std::io::Error),
}

/// Check if the current process is running inside a Docker container.
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
/// for details.
///
/// Adapted from <https://github.com/open-telemetry/opentelemetry-java-instrumentation/pull/7167/files>
/// Uses `/proc/self/cgroup` for cgroup v1,
/// uses `/proc/self/mountinfo` for cgroup v2
fn current_container_id() -> Result<String> {
    current_container_id_from_paths("/proc/self/cgroup", "/proc/self/mountinfo")
}

fn current_container_id_from_paths(
    cgroup_path: impl AsRef<Path>,
    mountinfo_path: impl AsRef<Path>,
) -> Result<String> {
    if let Ok(container_id) = container_id_from_cgroup_v1(cgroup_path) {
        return Ok(container_id);
    }
    container_id_from_cgroup_v2(mountinfo_path)
}

fn container_id_from_cgroup_v1(cgroup: impl AsRef<Path>) -> Result<String> {
    let content = fs::read_to_string(cgroup).context("Failed to read cgroup v1 info")?;
    content
        .lines()
        .find_map(parse_id_from_line)
        .context("Failed to detect Docker container id from cgroup v1")
}

fn parse_id_from_line(line: &str) -> Option<String> {
    let last_slash_idx = line.rfind('/')?;

    let last_section = &line[last_slash_idx + 1..];

    let container_id = if let Some(colon_idx) = last_section.rfind(':') {
        // Since containerd v1.5.0+, containerId is divided by the last colon when the
        // cgroupDriver is systemd:
        // https://github.com/containerd/containerd/blob/release/1.5/pkg/cri/server/helpers_linux.go#L64
        last_section[colon_idx + 1..].to_string()
    } else {
        let start_idx = last_section.rfind('-').map(|i| i + 1).unwrap_or(0);
        let end_idx = last_section.rfind('.').unwrap_or(last_section.len());

        if start_idx > end_idx {
            return None;
        }

        last_section[start_idx..end_idx].to_string()
    };

    if container_id.len() == 64 && container_id.chars().all(|c| c.is_ascii_hexdigit()) {
        return Some(container_id);
    }
    None
}

fn container_id_from_cgroup_v2(mount_info: impl AsRef<Path>) -> Result<String> {
    let content = fs::read_to_string(mount_info).context("Failed to read cgroup v2 mount info")?;
    regex!(r".*/(containers|overlay-containers)/([0-9a-f]{64})/.*")
        .captures(&content)
        .and_then(|caps| caps.get(2))
        .map(|m| m.as_str().to_owned())
        .context("Failed to find Docker container id in cgroup v2 mount info")
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
enum RuntimeKind {
    Auto,
    Docker,
    Podman,
}

impl FromStr for RuntimeKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "docker" => Ok(RuntimeKind::Docker),
            "podman" => Ok(RuntimeKind::Podman),
            "auto" => Ok(RuntimeKind::Auto),
            _ => Err(format!("Invalid container runtime: {s}")),
        }
    }
}

#[derive(serde::Deserialize, Debug)]
struct Mount {
    #[serde(rename = "Source")]
    source: String,
    #[serde(rename = "Destination")]
    destination: String,
}

impl RuntimeKind {
    fn cmd(&self) -> &str {
        match self {
            RuntimeKind::Docker => "docker",
            RuntimeKind::Podman => "podman",
            RuntimeKind::Auto => unreachable!("Auto should be resolved before use"),
        }
    }

    /// Detect if the current runtime is rootless.
    fn detect_rootless(self) -> Result<bool> {
        match self {
            RuntimeKind::Docker => {
                let output = Command::new(self.cmd())
                    .arg("info")
                    .arg("--format")
                    .arg("'{{ .SecurityOptions }}'")
                    .output()?;

                let stdout = str::from_utf8(&output.stdout)?;
                Ok(stdout.contains("name=rootless"))
            }
            RuntimeKind::Podman => {
                let output = Command::new(self.cmd())
                    .arg("info")
                    .arg("--format")
                    .arg("'{{ .Host.Security.Rootless }}'")
                    .output()?;

                let stdout = str::from_utf8(&output.stdout)?;
                Ok(stdout.eq_ignore_ascii_case("true"))
            }
            RuntimeKind::Auto => unreachable!("Auto should be resolved before use"),
        }
    }

    /// List the mounts of the current container.
    fn list_mounts(self) -> Result<Vec<Mount>> {
        if !is_in_docker() {
            anyhow::bail!("Not in a container");
        }

        let container_id = current_container_id()?;
        trace!(?container_id, "In Docker container");

        let output = Command::new(self.cmd())
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
    }
}

struct ContainerRuntimeInfo {
    runtime: RuntimeKind,
    rootless: bool,
    mounts: Vec<Mount>,
}

impl ContainerRuntimeInfo {
    /// Detect container runtime provider, prioritise docker over podman if
    /// both are on the path, unless `PREK_CONTAINER_RUNTIME` is set to override detection.
    fn resolve_runtime_kind<DF, PF>(
        env_override: Option<String>,
        docker_available: DF,
        podman_available: PF,
    ) -> RuntimeKind
    where
        DF: Fn() -> bool,
        PF: Fn() -> bool,
    {
        if let Some(val) = env_override {
            match RuntimeKind::from_str(&val) {
                Ok(runtime) => {
                    if runtime != RuntimeKind::Auto {
                        debug!(
                            "Container runtime overridden by {}={}",
                            EnvVars::PREK_CONTAINER_RUNTIME,
                            val
                        );
                        return runtime;
                    }
                }
                Err(_) => {
                    warn!(
                        "Invalid value for {}: {}, falling back to auto detection",
                        EnvVars::PREK_CONTAINER_RUNTIME,
                        val
                    );
                }
            }
        }

        if docker_available() {
            return RuntimeKind::Docker;
        }
        if podman_available() {
            return RuntimeKind::Podman;
        }

        debug!("No container runtime found on PATH, defaulting to docker");
        RuntimeKind::Docker
    }

    fn detect_runtime() -> Self {
        let runtime = Self::resolve_runtime_kind(
            EnvVars::var(EnvVars::PREK_CONTAINER_RUNTIME).ok(),
            || which::which("docker").is_ok(),
            || which::which("podman").is_ok(),
        );
        let rootless = runtime.detect_rootless().unwrap_or_else(|e| {
            warn!("Failed to detect if container runtime is rootless: {e}, defaulting to rootful");
            false
        });
        let mounts = runtime.list_mounts().unwrap_or_else(|e| {
            warn!("Failed to get container mounts: {e}, assuming no mounts");
            vec![]
        });

        Self {
            runtime,
            rootless,
            mounts,
        }
    }

    /// Get the command name of the container runtime.
    fn cmd(&self) -> &str {
        self.runtime.cmd()
    }

    fn is_rootless(&self) -> bool {
        self.rootless
    }

    fn is_podman(&self) -> bool {
        self.runtime == RuntimeKind::Podman
    }

    /// Get the path of the current directory in the host.
    fn map_to_host_path<'a>(&self, path: &'a Path) -> Cow<'a, Path> {
        for mount in &self.mounts {
            if let Ok(suffix) = path.strip_prefix(&mount.destination) {
                if suffix.components().next().is_none() {
                    // Exact match
                    return Cow::Owned(PathBuf::from(&mount.source));
                }
                let path = Path::new(&mount.source).join(suffix);
                return Cow::Owned(path);
            }
        }

        Cow::Borrowed(path)
    }
}

static CONTAINER_RUNTIME: LazyLock<ContainerRuntimeInfo> =
    LazyLock::new(ContainerRuntimeInfo::detect_runtime);

impl Docker {
    fn docker_tag(info: &InstallInfo) -> String {
        let mut hasher = DefaultHasher::new();

        info.language.hash(&mut hasher);
        info.language_version.hash(&mut hasher);
        let deps = info.dependencies.iter().collect::<BTreeSet<&String>>();
        deps.hash(&mut hasher);

        let digest = hex::encode(hasher.finish().to_le_bytes());
        format!("prek-{digest}")
    }

    async fn build_docker_image(
        hook: &Hook,
        install_info: &InstallInfo,
        pull: bool,
    ) -> Result<String> {
        let Some(src) = hook.repo_path() else {
            anyhow::bail!("Language `docker` cannot work with `local` repository");
        };

        let tag = Self::docker_tag(install_info);
        let mut cmd = Cmd::new(CONTAINER_RUNTIME.cmd(), "build docker image");
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

    pub(crate) fn docker_run_cmd(work_dir: &Path) -> Cmd {
        let mut command = Cmd::new(CONTAINER_RUNTIME.cmd(), "run container");
        command.arg("run").arg("--rm");

        if *USE_COLOR {
            command.arg("--tty");
        }

        // Run as a non-root user
        #[cfg(unix)]
        {
            let add_user_args = |cmd: &mut Cmd| {
                let uid = unsafe { libc::geteuid() };
                let gid = unsafe { libc::getegid() };
                cmd.arg("--user").arg(format!("{uid}:{gid}"));
            };

            // If runtime is rootful, set user to non-root user id matching current user id.
            if !CONTAINER_RUNTIME.is_rootless() {
                add_user_args(&mut command);
            } else if CONTAINER_RUNTIME.is_podman() {
                // For rootless podman, set user to non-root use id matching
                // current user id and add additional `--userns` param to map the user id correctly.
                add_user_args(&mut command);
                command.arg("--userns").arg("keep-id");
            }

            // Otherwise (rootless Docker): do nothing as it will cause permission
            // problems with bind mounted files.  In this state, `root:root` inside the container is
            // the same as current `uid:gid` on the host - see subuid / subgid.
        }

        let work_dir = CONTAINER_RUNTIME.map_to_host_path(work_dir);
        command
            // https://docs.docker.com/engine/reference/commandline/run/#mount-volumes-from-container-volumes-from
            // The `Z` option tells Docker to label the content with a private
            // unshared label. Only the current container can use a private volume.
            .arg("--volume")
            .arg(format!("{}:/src:rw,Z", work_dir.display()))
            // Run an init inside the container that forwards signals and reaps processes
            .arg("--init")
            .arg("--workdir")
            .arg("/src");

        command
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

        let mut info = InstallInfo::new(
            hook.language,
            hook.dependencies().clone(),
            &store.hooks_dir(),
        )?;

        Docker::build_docker_image(&hook, &info, true)
            .await
            .context("Failed to build docker image")?;

        info.persist_env_path();

        reporter.on_install_complete(progress);

        Ok(InstalledHook::Installed {
            hook,
            info: Arc::new(info),
        })
    }

    async fn check_health(&self, _info: &InstallInfo) -> Result<()> {
        Ok(())
    }

    async fn run(
        &self,
        hook: &InstalledHook,
        filenames: &[&Path],
        _store: &Store,
    ) -> Result<(i32, Vec<u8>)> {
        let docker_tag = Docker::build_docker_image(
            hook,
            hook.install_info().expect("Docker env must be installed"),
            false,
        )
        .await
        .context("Failed to build docker image")?;
        let entry = hook.entry.resolve(None)?;

        let run = async |batch: &[&Path]| {
            // docker run [OPTIONS] IMAGE [COMMAND] [ARG...]
            let mut cmd = Docker::docker_run_cmd(hook.work_dir());
            let mut output = cmd
                .current_dir(hook.work_dir())
                .arg("--entrypoint")
                .arg(&entry[0])
                .arg(&docker_tag)
                .args(&entry[1..])
                .args(&hook.args)
                .args(batch)
                .check(false)
                .stdin(Stdio::null())
                .output()
                .await?;

            output.stdout.extend(output.stderr);
            let code = output.status.code().unwrap_or(1);
            anyhow::Ok((code, output.stdout))
        };

        let results = run_by_batch(hook, filenames, &entry, run).await?;

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
    use super::*;
    use pretty_assertions::assert_eq;
    use std::io::Write;

    const CONTAINER_ID_V1: &str =
        "7be92808767a667f35c8505cbf40d14e931ef6db5b0210329cf193b15ba9d605";
    const CGROUP_V1_SAMPLE: &str = r"9:cpuset:/system.slice/docker-7be92808767a667f35c8505cbf40d14e931ef6db5b0210329cf193b15ba9d605.scope
8:cpuacct:/system.slice/docker-7be92808767a667f35c8505cbf40d14e931ef6db5b0210329cf193b15ba9d605.scope
";

    const CONTAINER_ID_V2: &str =
        "6d81fc3a1c26e24a27803e263d534be37c821e390521961a77f782c46fd85bc0";
    const MOUNTINFO_SAMPLE: &str = r"402 401 0:45 /docker/containers/6d81fc3a1c26e24a27803e263d534be37c821e390521961a77f782c46fd85bc0/hostname /etc/hostname rw,nosuid,nodev,relatime - tmpfs tmpfs rw,size=65536k,mode=755
403 401 0:45 /docker/containers/6d81fc3a1c26e24a27803e263d534be37c821e390521961a77f782c46fd85bc0/resolv.conf /etc/resolv.conf rw,nosuid,nodev,relatime - tmpfs tmpfs rw,size=65536k,mode=755
";

    #[test]
    fn test_container_id_from_cgroup_v1() -> anyhow::Result<()> {
        for (sample, expected) in [
            // with suffix
            (CGROUP_V1_SAMPLE, CONTAINER_ID_V1),
            // with prefix and suffix
            (
                "13:name=systemd:/podruntime/docker/kubepods/crio-dc679f8a8319c8cf7d38e1adf263bc08d234f0749ea715fb6ca3bb259db69956.stuff",
                "dc679f8a8319c8cf7d38e1adf263bc08d234f0749ea715fb6ca3bb259db69956",
            ),
            // just container id
            (
                "13:name=systemd:/pod/d86d75589bf6cc254f3e2cc29debdf85dde404998aa128997a819ff991827356",
                "d86d75589bf6cc254f3e2cc29debdf85dde404998aa128997a819ff991827356",
            ),
            // with prefix
            (
                "//\n1:name=systemd:/podruntime/docker/kubepods/docker-dc579f8a8319c8cf7d38e1adf263bc08d230600179b07acfd7eaf9646778dc31",
                "dc579f8a8319c8cf7d38e1adf263bc08d230600179b07acfd7eaf9646778dc31",
            ),
            // with two dashes in prefix
            (
                "11:perf_event:/kubepods.slice/kubepods-burstable.slice/kubepods-burstable-pod4415fd05_2c0f_4533_909b_f2180dca8d7c.slice/cri-containerd-713a77a26fe2a38ebebd5709604a048c3d380db1eb16aa43aca0b2499e54733c.scope",
                "713a77a26fe2a38ebebd5709604a048c3d380db1eb16aa43aca0b2499e54733c",
            ),
            // with colon
            (
                "11:devices:/system.slice/containerd.service/kubepods-pod87a18a64_b74a_454a_b10b_a4a36059d0a3.slice:cri-containerd:05c48c82caff3be3d7f1e896981dd410e81487538936914f32b624d168de9db0",
                "05c48c82caff3be3d7f1e896981dd410e81487538936914f32b624d168de9db0",
            ),
        ] {
            let mut cgroup_file = tempfile::NamedTempFile::new()?;
            cgroup_file.write_all(sample.as_bytes())?;
            cgroup_file.flush()?;

            let actual = container_id_from_cgroup_v1(cgroup_file.path())?;
            assert_eq!(actual, expected);
        }

        Ok(())
    }

    #[test]
    fn invalid_container_id_from_cgroup_v1() -> anyhow::Result<()> {
        for sample in [
            // Too short
            "9:cpuset:/system.slice/docker-7be92808767a667f35c8505cbf40d14e931ef6db5b0210329cf193b15ba9d60.scope",
            // Non-hex characters
            "9:cpuset:/system.slice/docker-7be92808767a667f35c8505cbf40d14e931ef6db5b0210329cf193b15ba9d6g0.scope",
            // No container id
            "9:cpuset:/system.slice/docker-.scope",
        ] {
            let mut cgroup_file = tempfile::NamedTempFile::new()?;
            cgroup_file.write_all(sample.as_bytes())?;
            cgroup_file.flush()?;

            let result = container_id_from_cgroup_v1(cgroup_file.path());
            assert!(result.is_err());
        }

        Ok(())
    }

    #[test]
    fn test_container_id_from_cgroup_v2() -> anyhow::Result<()> {
        for (sample, expected) in [
            // Docker rootful container
            (
                r"402 401 0:45 /var/lib/docker/containers/6d81fc3a1c26e24a27803e263d534be37c821e390521961a77f782c46fd85bc0/hostname /etc/hostname rw,nosuid,nodev,relatime - tmpfs tmpfs rw,size=65536k,mode=755
403 401 0:45 /var/lib/docker/containers/6d81fc3a1c26e24a27803e263d534be37c821e390521961a77f782c46fd85bc0/resolv.conf /etc/resolv.conf rw,nosuid,nodev,relatime - tmpfs tmpfs rw,size=65536k,mode=755
",
                "6d81fc3a1c26e24a27803e263d534be37c821e390521961a77f782c46fd85bc0",
            ),
            // Docker rootless container
            (
                r"402 401 0:45 /home/testuser/.local/share/docker/containers/6d81fc3a1c26e24a27803e263d534be37c821e390521961a77f782c46fd85bc1/hostname /etc/hostname rw,nosuid,nodev,relatime - tmpfs tmpfs rw,size=65536k,mode=755
403 401 0:45 /home/testuser/.local/share/docker/containers/6d81fc3a1c26e24a27803e263d534be37c821e390521961a77f782c46fd85bc1/resolv.conf /etc/resolv.conf rw,nosuid,nodev,relatime - tmpfs tmpfs rw,size=65536k,mode=755
",
                "6d81fc3a1c26e24a27803e263d534be37c821e390521961a77f782c46fd85bc1",
            ),
            // Podman rootful container
            (
                r"1099 1105 0:107 /containers/storage/overlay-containers/6d81fc3a1c26e24a27803e263d534be37c821e390521961a77f782c46fd85bc2/userdata/hostname /etc/hostname rw,nosuid,nodev,relatime - tmpfs tmpfs rw,seclabel,size=3256724k,nr_inodes=814181,mode=700,uid=1000,gid=1000,inode64
1100 1105 0:107 /containers/storage/overlay-containers/6d81fc3a1c26e24a27803e263d534be37c821e390521961a77f782c46fd85bc2/userdata/resolv.conf /etc/resolv.conf rw,nosuid,nodev,relatime - tmpfs tmpfs rw,seclabel,size=3256724k,nr_inodes=814181,mode=700,uid=1000,gid=1000,inode64
",
                "6d81fc3a1c26e24a27803e263d534be37c821e390521961a77f782c46fd85bc2",
            ),
            // Podman rootless container
            (
                r"1099 1105 0:107 /containers/overlay-containers/6d81fc3a1c26e24a27803e263d534be37c821e390521961a77f782c46fd85bc3/userdata/hostname /etc/hostname rw,nosuid,nodev,relatime - tmpfs tmpfs rw,seclabel,size=3256724k,nr_inodes=814181,mode=700,uid=1000,gid=1000,inode64
1100 1105 0:107 /containers/overlay-containers/6d81fc3a1c26e24a27803e263d534be37c821e390521961a77f782c46fd85bc3/userdata/resolv.conf /etc/resolv.conf rw,nosuid,nodev,relatime - tmpfs tmpfs rw,seclabel,size=3256724k,nr_inodes=814181,mode=700,uid=1000,gid=1000,inode64
",
                "6d81fc3a1c26e24a27803e263d534be37c821e390521961a77f782c46fd85bc3",
            ),
        ] {
            let mut mountinfo_file = tempfile::NamedTempFile::new()?;
            mountinfo_file.write_all(sample.as_bytes())?;
            mountinfo_file.flush()?;

            let actual = container_id_from_cgroup_v2(mountinfo_file.path())?;
            assert_eq!(actual, expected);
        }
        Ok(())
    }

    #[test]
    fn test_current_container_id_prefers_cgroup_v1() -> anyhow::Result<()> {
        let mut cgroup_file = tempfile::NamedTempFile::new()?;
        let mut mountinfo_file = tempfile::NamedTempFile::new()?;
        cgroup_file.write_all(CGROUP_V1_SAMPLE.as_bytes())?;
        mountinfo_file.write_all(MOUNTINFO_SAMPLE.as_bytes())?;
        cgroup_file.flush()?;
        mountinfo_file.flush()?;

        let container_id =
            current_container_id_from_paths(cgroup_file.path(), mountinfo_file.path())?;
        assert_eq!(container_id, CONTAINER_ID_V1);
        Ok(())
    }

    #[test]
    fn test_current_container_id_falls_back_to_cgroup_v2() -> anyhow::Result<()> {
        let mut cgroup_file = tempfile::NamedTempFile::new()?;
        let mut mountinfo_file = tempfile::NamedTempFile::new()?;
        cgroup_file.write_all(b"0::/\n")?; // No cgroup v1 container id available.
        mountinfo_file.write_all(MOUNTINFO_SAMPLE.as_bytes())?;
        cgroup_file.flush()?;
        mountinfo_file.flush()?;

        let container_id =
            current_container_id_from_paths(cgroup_file.path(), mountinfo_file.path())?;
        assert_eq!(container_id, CONTAINER_ID_V2);
        Ok(())
    }

    #[test]
    fn test_current_container_id_errors_when_no_match() -> anyhow::Result<()> {
        let cgroup_file = tempfile::NamedTempFile::new()?;
        let mut mountinfo_file = tempfile::NamedTempFile::new()?;
        mountinfo_file.write_all(b"501 500 0:45 /proc /proc rw\n")?;
        mountinfo_file.flush()?;

        let result = current_container_id_from_paths(cgroup_file.path(), mountinfo_file.path());
        assert!(result.is_err());

        Ok(())
    }

    #[test]
    fn test_detect_container_runtime() {
        fn runtime_with(
            env_override: Option<&str>,
            docker_available: bool,
            podman_available: bool,
        ) -> RuntimeKind {
            ContainerRuntimeInfo::resolve_runtime_kind(
                env_override.map(ToString::to_string),
                || docker_available,
                || podman_available,
            )
        }

        assert_eq!(runtime_with(None, true, false), RuntimeKind::Docker);
        assert_eq!(runtime_with(None, false, true), RuntimeKind::Podman);
        assert_eq!(runtime_with(None, false, false), RuntimeKind::Docker);

        assert_eq!(runtime_with(Some("auto"), true, false), RuntimeKind::Docker);
        assert_eq!(runtime_with(Some("auto"), false, true), RuntimeKind::Podman);
        assert_eq!(
            runtime_with(Some("auto"), false, false),
            RuntimeKind::Docker
        );

        assert_eq!(
            runtime_with(Some("docker"), true, false),
            RuntimeKind::Docker
        );
        assert_eq!(
            runtime_with(Some("docker"), false, true),
            RuntimeKind::Docker
        );
        assert_eq!(
            runtime_with(Some("DOCKER"), false, false),
            RuntimeKind::Docker
        );
        assert_eq!(
            runtime_with(Some("podman"), true, false),
            RuntimeKind::Podman
        );
        assert_eq!(
            runtime_with(Some("podman"), false, true),
            RuntimeKind::Podman
        );
        assert_eq!(
            runtime_with(Some("podman"), false, false),
            RuntimeKind::Podman
        );

        assert_eq!(
            runtime_with(Some("invalid"), false, false),
            RuntimeKind::Docker
        );
    }
}
