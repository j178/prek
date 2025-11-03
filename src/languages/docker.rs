use std::borrow::Cow;
use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::Read;
use std::path::Path;
use std::sync::{Arc, LazyLock};

use anyhow::{Context, Result};
use lazy_regex::regex;
use tracing::trace;

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

    let Ok(container_id) = Docker::current_container_id(None, None) else {
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
    /// uses /proc/self/cgroup for cgroup v1
    /// uses /proc/self/mountinfo for cgroup v2
    fn current_container_id(
        cgroup: Option<fs::File>,
        mountinfo: Option<fs::File>,
    ) -> Result<String> {
        // Adapted from https://github.com/open-telemetry/opentelemetry-java-instrumentation/pull/7167/files
        let regex = regex!(r".*/([0-9a-f]{64}).*");

        let cgroup_path = if let Some(mut file) = cgroup {
            let mut buffer: Vec<u8> = Vec::new();
            file.read_to_end(&mut buffer)?;
            String::from_utf8(buffer)?
        } else {
            fs::read_to_string("/proc/self/cgroup")?
        };

        let mount_info = if let Some(mut file) = mountinfo {
            let mut buffer: Vec<u8> = Vec::new();
            file.read_to_end(&mut buffer)?;
            String::from_utf8(buffer)?
        } else {
            fs::read_to_string("/proc/self/mountinfo")?
        };

        // mountinfo seems to be more reliable when running in a container using cgroups v2
        let captures = if let Some(captures) = regex.captures(&cgroup_path) {
            captures
        } else if let Some(captures) = regex.captures(&mount_info) {
            captures
        } else {
            anyhow::bail!("Failed to get container id: no match found regex point");
        };

        let Some(id) = captures.get(1).map(|m| m.as_str().to_string()) else {
            anyhow::bail!("Failed to get container id: no capture found");
        };
        Ok(id)
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
mod test {
    use std::io::{Seek, Write};

    use pretty_assertions::assert_str_eq;

    use super::*;

    #[test]
    fn test_detect_dind_no_match() {
        let mut cgroup = tempfile::tempfile().expect("cannot create tempfile");
        let mut mountinfo = tempfile::tempfile().expect("cannot create tempfile");
        cgroup.write_all(b"0::/\n").expect("cannot write data");
        mountinfo
            .write_all(
                br"
1338 1104 0:90 /asound /proc/asound ro,nosuid,nodev,noexec,relatime - proc proc rw
1339 1104 0:90 /bus /proc/bus ro,nosuid,nodev,noexec,relatime - proc proc rw
1340 1104 0:90 /fs /proc/fs ro,nosuid,nodev,noexec,relatime - proc proc rw
1341 1104 0:90 /irq /proc/irq ro,nosuid,nodev,noexec,relatime - proc proc rw
1342 1104 0:90 /sys /proc/sys ro,nosuid,nodev,noexec,relatime - proc proc rw
1343 1104 0:90 /sysrq-trigger /proc/sysrq-trigger ro,nosuid,nodev,noexec,relatime - proc proc rw
        ",
            )
            .expect("cannot write data");

        cgroup.rewind().expect("could not rewind file");
        mountinfo.rewind().expect("could not rewind file");
        assert!(Docker::current_container_id(Some(cgroup), Some(mountinfo)).is_err());
    }

    #[test]
    fn test_detect_dind_cgroup_v1() {
        let mut cgroup = tempfile::tempfile().expect("cannot create tempfile");
        let mut mountinfo = tempfile::tempfile().expect("cannot create tempfile");
        cgroup
            .write_all(
                br"
3:cpu:/docker/7be92808767a667f35c8505cbf40d14e931ef6db5b0210329cf193b15ba9d605
2:cpuset:/docker/7be92808767a667f35c8505cbf40d14e931ef6db5b0210329cf193b15ba9d605
1:name=openrc:/docker
            ",
            )
            .expect("could not write to file");
        cgroup.rewind().expect("could not rewind file");

        mountinfo.write_all(br"
1087 1093 0:106 /containers/overlay-containers/6d81fc3a1c26e24a27803e263d534be37c821e390521961a77f782c46fd85bc0/userdata/resolv.conf /etc/resolv.conf rw,nosuid,nodev,relatime - tmpfs tmpfs rw,seclabel,size=3256724k,nr_inodes=814181,mode=700,uid=1000,gid=1000,inode64
1088 1093 0:106 /containers/overlay-containers/6d81fc3a1c26e24a27803e263d534be37c821e390521961a77f782c46fd85bc0/userdata/hosts /etc/hosts rw,nosuid,nodev,relatime - tmpfs tmpfs rw,seclabel,size=3256724k,nr_inodes=814181,mode=700,uid=1000,gid=1000,inode64
1090 1093 0:106 /containers/overlay-containers/6d81fc3a1c26e24a27803e263d534be37c821e390521961a77f782c46fd85bc0/userdata/.containerenv /run/.containerenv rw,nosuid,nodev,relatime - tmpfs tmpfs rw,seclabel,size=3256724k,nr_inodes=814181,mode=700,uid=1000,gid=1000,inode64
1091 1093 0:106 /containers/overlay-containers/6d81fc3a1c26e24a27803e263d534be37c821e390521961a77f782c46fd85bc0/userdata/run/secrets /run/secrets rw,nosuid,nodev,relatime - tmpfs tmpfs rw,seclabel,size=3256724k,nr_inodes=814181,mode=700,uid=1000,gid=1000,inode64
1092 1093 0:106 /containers/overlay-containers/6d81fc3a1c26e24a27803e263d534be37c821e390521961a77f782c46fd85bc0/userdata/hostname /etc/hostname rw,nosuid,nodev,relatime - tmpfs tmpfs rw,seclabel,size=3256724k,nr_inodes=814181,mode=700,uid=1000,gid=1000,inode64
        ").expect("could not write to file");
        mountinfo.rewind().expect("could not rewind file");

        let container_id = Docker::current_container_id(Some(cgroup), Some(mountinfo)).unwrap();
        assert_str_eq!(
            container_id.as_str(),
            "7be92808767a667f35c8505cbf40d14e931ef6db5b0210329cf193b15ba9d605"
        );
    }

    #[test]
    fn test_detect_dind_cgroup_v2() {
        let mut cgroup = tempfile::tempfile().expect("cannot create tempfile");
        let mut mountinfo = tempfile::tempfile().expect("cannot create tempfile");
        cgroup.write_all(b"0::/\n").expect("cannot write data");
        mountinfo.write_all(br"
1087 1093 0:106 /containers/overlay-containers/6d81fc3a1c26e24a27803e263d534be37c821e390521961a77f782c46fd85bc0/userdata/resolv.conf /etc/resolv.conf rw,nosuid,nodev,relatime - tmpfs tmpfs rw,seclabel,size=3256724k,nr_inodes=814181,mode=700,uid=1000,gid=1000,inode64
1088 1093 0:106 /containers/overlay-containers/6d81fc3a1c26e24a27803e263d534be37c821e390521961a77f782c46fd85bc0/userdata/hosts /etc/hosts rw,nosuid,nodev,relatime - tmpfs tmpfs rw,seclabel,size=3256724k,nr_inodes=814181,mode=700,uid=1000,gid=1000,inode64
1090 1093 0:106 /containers/overlay-containers/6d81fc3a1c26e24a27803e263d534be37c821e390521961a77f782c46fd85bc0/userdata/.containerenv /run/.containerenv rw,nosuid,nodev,relatime - tmpfs tmpfs rw,seclabel,size=3256724k,nr_inodes=814181,mode=700,uid=1000,gid=1000,inode64
1091 1093 0:106 /containers/overlay-containers/6d81fc3a1c26e24a27803e263d534be37c821e390521961a77f782c46fd85bc0/userdata/run/secrets /run/secrets rw,nosuid,nodev,relatime - tmpfs tmpfs rw,seclabel,size=3256724k,nr_inodes=814181,mode=700,uid=1000,gid=1000,inode64
1092 1093 0:106 /containers/overlay-containers/6d81fc3a1c26e24a27803e263d534be37c821e390521961a77f782c46fd85bc0/userdata/hostname /etc/hostname rw,nosuid,nodev,relatime - tmpfs tmpfs rw,seclabel,size=3256724k,nr_inodes=814181,mode=700,uid=1000,gid=1000,inode64
        ").expect("cannot write data");

        cgroup.rewind().expect("could not rewind file");
        mountinfo.rewind().expect("could not rewind file");
        let container_id = Docker::current_container_id(Some(cgroup), Some(mountinfo)).unwrap();
        assert_str_eq!(
            container_id.as_str(),
            "6d81fc3a1c26e24a27803e263d534be37c821e390521961a77f782c46fd85bc0"
        );
    }
}
