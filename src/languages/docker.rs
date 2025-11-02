use std::borrow::Cow;
use std::collections::hash_map::DefaultHasher;
use std::fmt::Display;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::Path;
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

#[derive(Debug, Copy, Clone, Default, PartialEq, Eq)]
pub(crate) enum ContainerRuntime {
    Auto,
    #[default]
    Docker,
    Podman,
}

impl FromStr for ContainerRuntime {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "docker" => Ok(ContainerRuntime::Docker),
            "podman" => Ok(ContainerRuntime::Podman),
            "auto" => Ok(ContainerRuntime::Auto),
            _ => Err(format!("Invalid container runtime: {s}")),
        }
    }
}

impl ContainerRuntime {
    fn cmd(&self) -> &str {
        match self {
            ContainerRuntime::Docker => "docker",
            ContainerRuntime::Podman => "podman",
            ContainerRuntime::Auto => unreachable!("Auto should be resolved before use"),
        }
    }
}

impl Display for ContainerRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ContainerRuntime::Docker => write!(f, "docker"),
            ContainerRuntime::Podman => write!(f, "podman"),
            ContainerRuntime::Auto => write!(f, "auto"),
        }
    }
}

static CONTAINER_RUNTIME: LazyLock<ContainerRuntime> = LazyLock::new(|| {
    detect_container_runtime(
        EnvVars::var(EnvVars::PREK_CONTAINER_RUNTIME).ok(),
        || which::which("docker").is_ok(),
        || which::which("podman").is_ok(),
    )
});

/// Detect container runtime provider, prioritise docker over podman if
/// both are on the path, unless `PREK_CONTAINER_RUNTIME` is set to override detection.
fn detect_container_runtime<DF, PF>(
    env_override: Option<String>,
    docker_available: DF,
    podman_available: PF,
) -> ContainerRuntime
where
    DF: Fn() -> bool,
    PF: Fn() -> bool,
{
    if let Some(val) = env_override {
        match ContainerRuntime::from_str(&val) {
            Ok(runtime) => {
                if runtime != ContainerRuntime::Auto {
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
        return ContainerRuntime::Docker;
    }
    if podman_available() {
        return ContainerRuntime::Podman;
    }

    let runtime = ContainerRuntime::default();
    debug!("No container runtime found on PATH, defaulting to {runtime}");
    runtime
}

static CONTAINER_RUNTIME_ROOTLESS: LazyLock<ContainerRuntimeRootless> = LazyLock::new(|| {
    detect_container_runtime_rootless(
        *CONTAINER_RUNTIME,
        EnvVars::var_as_bool(EnvVars::PREK_CONTAINER_RUNTIME_ROOTLESS),
        detect_rootless,
        detect_rootless_command,
    )
});

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub(crate) enum ContainerRuntimeRootless {
    Rootless,
    Rootful,
}

fn detect_container_runtime_rootless<DR, DRC>(
    runtime: ContainerRuntime,
    env_override: Option<bool>,
    rootless: DR,
    rootless_command: DRC,
) -> ContainerRuntimeRootless
where
    DR: Fn(ContainerRuntime, DRC) -> ContainerRuntimeRootless,
    DRC: Fn(ContainerRuntime) -> std::process::Command,
{
    // if env var set then return value in line with setting
    if let Some(val) = env_override {
        if val {
            return ContainerRuntimeRootless::Rootless;
        }
        return ContainerRuntimeRootless::Rootful;
    }

    rootless(runtime, rootless_command)
}

// Attempt to detect if current runtime is rootless or not, if unable to detect for docker runtime
// it will default to rootful and for podman runtime it will default to rootless.  This matches the
// default installations for the respective runtimes
fn detect_rootless<DRC>(runtime: ContainerRuntime, drc: DRC) -> ContainerRuntimeRootless
where
    DRC: Fn(ContainerRuntime) -> std::process::Command,
{
    let mut command = drc(runtime);

    let output = match command.output() {
        Ok(output) => String::from_utf8_lossy(&output.stdout)
            .trim()
            .to_owned()
            .replace('\'', ""),
        Err(_e) => {
            if runtime == ContainerRuntime::Podman {
                return ContainerRuntimeRootless::Rootless;
            }
            return ContainerRuntimeRootless::Rootful;
        }
    };

    match runtime {
        ContainerRuntime::Podman => {
            if output.eq_ignore_ascii_case("true") {
                ContainerRuntimeRootless::Rootless
            } else {
                ContainerRuntimeRootless::Rootful
            }
        }
        ContainerRuntime::Docker => {
            if output.eq_ignore_ascii_case("rootless") {
                ContainerRuntimeRootless::Rootless
            } else {
                ContainerRuntimeRootless::Rootful
            }
        }
        ContainerRuntime::Auto => unreachable!("Auto should be resolved before use"),
    }
}

fn detect_rootless_command(runtime: ContainerRuntime) -> std::process::Command {
    let mut command = std::process::Command::new(runtime.cmd());
    command.arg("info");
    command.arg("--format");

    match runtime {
        ContainerRuntime::Podman => command.arg("'{{ .Host.Security.Rootless }}'"),
        ContainerRuntime::Docker => command.arg("'{{ .ClientInfo.Context }}'"),
        ContainerRuntime::Auto => unreachable!("Auto should be resolved before use"),
    };
    command
}

impl Docker {
    fn docker_tag(hook: &InstalledHook) -> String {
        let info = hook.install_info().expect("Docker hook must be installed");

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
        Self::current_container_id_from_paths("/proc/self/cgroup", "/proc/self/mountinfo")
    }

    fn current_container_id_from_paths(
        cgroup_path: impl AsRef<Path>,
        mountinfo_path: impl AsRef<Path>,
    ) -> Result<String> {
        if let Ok(container_id) = Self::container_id_from_cgroup_v1(cgroup_path) {
            return Ok(container_id);
        }
        Self::container_id_from_cgroup_v2(mountinfo_path)
    }

    fn container_id_from_cgroup_v1(cgroup: impl AsRef<Path>) -> Result<String> {
        let content = fs::read_to_string(cgroup).context("Failed to read cgroup v1 info")?;
        content
            .lines()
            .find_map(Docker::parse_id_from_line)
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
        let content =
            fs::read_to_string(mount_info).context("Failed to read cgroup v2 mount info")?;
        regex!(r".*/(containers|overlay-containers)/([0-9a-f]{64})/.*")
            .captures(&content)
            .and_then(|caps| caps.get(2))
            .map(|m| m.as_str().to_owned())
            .context("Failed to find Docker container id in cgroup v2 mount info")
    }

    /// Get the path of the current directory in the host.
    fn get_docker_path(path: &Path) -> Result<Cow<'_, Path>> {
        let mounts = CONTAINER_MOUNTS.as_ref()?;

        for mount in mounts {
            if let Ok(suffix) = path.strip_prefix(&mount.destination) {
                if suffix.components().next().is_none() {
                    // Exact match
                    return Ok(Path::new(&mount.source).into());
                }
                let path = Path::new(&mount.source).join(suffix);
                return Ok(path.into());
            }
        }

        Ok(path.into())
    }

    pub(crate) fn docker_run_cmd(work_dir: &Path) -> Result<Cmd> {
        let mut command = Cmd::new(CONTAINER_RUNTIME.cmd(), "run container");
        command.arg("run").arg("--rm");

        if *USE_COLOR {
            command.arg("--tty");
        }

        // Run as a non-root user
        #[cfg(unix)]
        {
            let user_mapping_args = || {
                vec![
                    "--user".to_owned(),
                    format!("{}:{}", unsafe { libc::geteuid() }, unsafe {
                        libc::getegid()
                    }),
                ]
            };

            // If runtime is rootful set user to non-root user id matching
            // current user id
            if *CONTAINER_RUNTIME_ROOTLESS == ContainerRuntimeRootless::Rootful {
                command.args(user_mapping_args());
            }

            // If rootless and runtime is podman, set user to non-root use id matching
            // current user id and add additional --userns param to map the user id correctly
            if *CONTAINER_RUNTIME == ContainerRuntime::Podman
                && *CONTAINER_RUNTIME_ROOTLESS == ContainerRuntimeRootless::Rootless
            {
                command.args(user_mapping_args());
                command.arg("--userns");
                command.arg("keep-id");
            }

            // if neither match of the above match then we are running docker in rootless mode,
            // when this is true do not perform any user mapping as it will cause permission
            // problems with bind mounted files.  in this state root:root inside the container is
            // the same as current uid:gid on the host - see subuid / subgid
        }

        let work_dir = Self::get_docker_path(work_dir)?;
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
        )?;
        let installed_hook = InstalledHook::Installed {
            hook,
            info: Arc::new(info),
        };

        Docker::build_docker_image(&installed_hook, true)
            .await
            .context("Failed to build docker image")?;

        reporter.on_install_complete(progress);

        Ok(installed_hook)
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
        let docker_tag = Docker::build_docker_image(hook, false)
            .await
            .context("Failed to build docker image")?;
        let entry = hook.entry.resolve(None)?;

        let run = async move |batch: &[&Path]| {
            // docker run [OPTIONS] IMAGE [COMMAND] [ARG...]
            let mut cmd = Docker::docker_run_cmd(hook.work_dir())?;
            let mut output = cmd
                .current_dir(hook.work_dir())
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

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use std::ffi::OsStr;

    use crate::languages::docker::ContainerRuntimeRootless;

    use super::{ContainerRuntime, Docker};
    use std::io::Write;

    // Real-world inspired samples captured from Docker hosts.
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
    fn container_id_from_cgroup_v1() -> anyhow::Result<()> {
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

            let actual = Docker::container_id_from_cgroup_v1(cgroup_file.path())?;
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

            let result = Docker::container_id_from_cgroup_v1(cgroup_file.path());
            assert!(result.is_err());
        }

        Ok(())
    }

    #[test]
    fn container_id_from_cgroup_v2() -> anyhow::Result<()> {
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

            let actual = Docker::container_id_from_cgroup_v2(mountinfo_file.path())?;
            assert_eq!(actual, expected);
        }
        Ok(())
    }

    #[test]
    fn current_container_id_prefers_cgroup_v1_samples() -> anyhow::Result<()> {
        let mut cgroup_file = tempfile::NamedTempFile::new()?;
        let mut mountinfo_file = tempfile::NamedTempFile::new()?;
        cgroup_file.write_all(CGROUP_V1_SAMPLE.as_bytes())?;
        mountinfo_file.write_all(MOUNTINFO_SAMPLE.as_bytes())?;
        cgroup_file.flush()?;
        mountinfo_file.flush()?;

        let container_id =
            Docker::current_container_id_from_paths(cgroup_file.path(), mountinfo_file.path())?;
        assert_eq!(container_id, CONTAINER_ID_V1);
        Ok(())
    }

    #[test]
    fn current_container_id_falls_back_to_cgroup_v2_samples() -> anyhow::Result<()> {
        let mut cgroup_file = tempfile::NamedTempFile::new()?;
        let mut mountinfo_file = tempfile::NamedTempFile::new()?;
        cgroup_file.write_all(b"0::/\n")?; // No cgroup v1 container id available.
        mountinfo_file.write_all(MOUNTINFO_SAMPLE.as_bytes())?;
        cgroup_file.flush()?;
        mountinfo_file.flush()?;

        let container_id =
            Docker::current_container_id_from_paths(cgroup_file.path(), mountinfo_file.path())?;
        assert_eq!(container_id, CONTAINER_ID_V2);
        Ok(())
    }

    #[test]
    fn current_container_id_errors_when_no_match() -> anyhow::Result<()> {
        let cgroup_file = tempfile::NamedTempFile::new()?;
        let mut mountinfo_file = tempfile::NamedTempFile::new()?;
        mountinfo_file.write_all(b"501 500 0:45 /proc /proc rw\n")?;
        mountinfo_file.flush()?;

        let result =
            Docker::current_container_id_from_paths(cgroup_file.path(), mountinfo_file.path());
        assert!(result.is_err());

        Ok(())
    }

    #[test]
    fn test_detect_container_runtime() {
        fn runtime_with(
            env_override: Option<&str>,
            docker_available: bool,
            podman_available: bool,
        ) -> ContainerRuntime {
            super::detect_container_runtime(
                env_override.map(ToString::to_string),
                || docker_available,
                || podman_available,
            )
        }

        assert_eq!(runtime_with(None, true, false), ContainerRuntime::Docker);
        assert_eq!(runtime_with(None, false, true), ContainerRuntime::Podman);
        assert_eq!(
            runtime_with(None, false, false),
            ContainerRuntime::default()
        );

        assert_eq!(
            runtime_with(Some("auto"), true, false),
            ContainerRuntime::Docker
        );
        assert_eq!(
            runtime_with(Some("auto"), false, true),
            ContainerRuntime::Podman
        );
        assert_eq!(
            runtime_with(Some("auto"), false, false),
            ContainerRuntime::default()
        );

        assert_eq!(
            runtime_with(Some("docker"), true, false),
            ContainerRuntime::Docker
        );
        assert_eq!(
            runtime_with(Some("docker"), false, true),
            ContainerRuntime::Docker
        );
        assert_eq!(
            runtime_with(Some("DOCKER"), false, false),
            ContainerRuntime::Docker
        );
        assert_eq!(
            runtime_with(Some("podman"), true, false),
            ContainerRuntime::Podman
        );
        assert_eq!(
            runtime_with(Some("podman"), false, true),
            ContainerRuntime::Podman
        );
        assert_eq!(
            runtime_with(Some("podman"), false, false),
            ContainerRuntime::Podman
        );

        assert_eq!(
            runtime_with(Some("invalid"), false, false),
            ContainerRuntime::Docker
        );
    }

    #[test]
    fn test_detect_rootless_command() {
        // Verify command docker
        let command = super::detect_rootless_command(ContainerRuntime::Docker);
        let args: Vec<&OsStr> = command.get_args().collect();
        assert_eq!(command.get_program(), "docker");
        assert_eq!(args, &["info", "--format", "'{{ .ClientInfo.Context }}'"]);

        // Verify command podman
        let command = super::detect_rootless_command(ContainerRuntime::Podman);
        let args: Vec<&OsStr> = command.get_args().collect();
        assert_eq!(command.get_program(), "podman");
        assert_eq!(
            args,
            &["info", "--format", "'{{ .Host.Security.Rootless }}'"]
        );
    }

    #[test]
    fn detect_rootless_runtime() {
        fn rootless(runtime: ContainerRuntime) -> std::process::Command {
            let mut command = std::process::Command::new("echo");
            match runtime {
                ContainerRuntime::Docker => command.arg("'rootless'"),
                ContainerRuntime::Podman => command.arg("'true'"),
                ContainerRuntime::Auto => unreachable!("Auto should be resolved before use"),
            };
            command
        }

        fn rootful(runtime: ContainerRuntime) -> std::process::Command {
            let mut command = std::process::Command::new("echo");
            match runtime {
                ContainerRuntime::Docker => command.arg("'root'"),
                ContainerRuntime::Podman => command.arg("'false'"),
                ContainerRuntime::Auto => unreachable!("Auto should be resolved before use"),
            };
            command
        }

        // podman detect rootless
        assert_eq!(
            super::detect_container_runtime_rootless(
                ContainerRuntime::Podman,
                None,
                super::detect_rootless,
                rootless
            ),
            ContainerRuntimeRootless::Rootless
        );

        // podman detect rootful
        assert_eq!(
            super::detect_container_runtime_rootless(
                ContainerRuntime::Podman,
                None,
                super::detect_rootless,
                rootful
            ),
            ContainerRuntimeRootless::Rootful
        );

        // docker detect rootless
        assert_eq!(
            super::detect_container_runtime_rootless(
                ContainerRuntime::Docker,
                None,
                super::detect_rootless,
                rootless
            ),
            ContainerRuntimeRootless::Rootless
        );

        // docker detect rootful
        assert_eq!(
            super::detect_container_runtime_rootless(
                ContainerRuntime::Docker,
                None,
                super::detect_rootless,
                rootful
            ),
            ContainerRuntimeRootless::Rootful
        );

        // podman force rootless
        assert_eq!(
            super::detect_container_runtime_rootless(
                ContainerRuntime::Podman,
                Some(true),
                super::detect_rootless,
                rootful
            ),
            ContainerRuntimeRootless::Rootless
        );

        // docker force rootless
        assert_eq!(
            super::detect_container_runtime_rootless(
                ContainerRuntime::Docker,
                Some(true),
                super::detect_rootless,
                rootful
            ),
            ContainerRuntimeRootless::Rootless
        );

        // podman force root
        assert_eq!(
            super::detect_container_runtime_rootless(
                ContainerRuntime::Podman,
                Some(false),
                super::detect_rootless,
                rootless
            ),
            ContainerRuntimeRootless::Rootful
        );

        // docker force root
        assert_eq!(
            super::detect_container_runtime_rootless(
                ContainerRuntime::Docker,
                Some(false),
                super::detect_rootless,
                rootful
            ),
            ContainerRuntimeRootless::Rootful
        );
    }
}
