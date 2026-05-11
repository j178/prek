use std::ffi::OsStr;
use std::fmt::{Display, Formatter};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;

use anyhow::Result;
use prek_consts::env_vars::EnvVars;
use prek_identify::parse_shebang;
use rustc_hash::FxHashSet;
use tracing::{instrument, trace};

use crate::cli::reporter::{HookInstallReporter, HookRunReporter};
use crate::config::Language;
use crate::fs::CWD;
use crate::hook::{Hook, InstalledHook, InstalledHookEnv, Repo};
use crate::hook_entry::HookEntry;
use crate::hooks;
use crate::languages::version::LanguageRequest;
use crate::store::{CacheBucket, Store, ToolBucket};

mod bun;
mod dart;
mod deno;
mod docker;
mod docker_image;
mod dotnet;
mod fail;
mod golang;
mod haskell;
mod julia;
mod lua;
mod node;
mod pygrep;
mod python;
mod ruby;
mod rust;
mod script;
mod swift;
mod system;
pub(crate) mod version;

trait LanguageImpl {
    async fn install(
        &self,
        hook: Arc<Hook>,
        store: &Store,
        reporter: &HookInstallReporter,
    ) -> Result<InstalledHook>;

    async fn check_health(&self, env: &InstalledHookEnv) -> Result<()>;

    async fn run(
        &self,
        hook: &InstalledHook,
        filenames: &[&Path],
        store: &Store,
        reporter: &HookRunReporter,
    ) -> Result<(i32, Vec<u8>)>;
}

pub(crate) struct HookMetadata<'a> {
    pub(crate) id: &'a str,
    pub(crate) language: Language,
    pub(crate) entry: &'a HookEntry,
    pub(crate) repo_path: Option<&'a Path>,
    pub(crate) work_dir: &'a Path,
    pub(crate) additional_dependencies: &'a mut FxHashSet<String>,
    pub(crate) language_request: &'a mut LanguageRequest,
}

impl Display for HookMetadata<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.id)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShellSupport {
    Supported,
    Unsupported(&'static str),
}

#[derive(thiserror::Error, Debug)]
#[error("Language `{0}` is not implemented yet")]
struct UnimplementedError(String);

struct Unimplemented;

impl LanguageImpl for Unimplemented {
    async fn install(
        &self,
        hook: Arc<Hook>,
        _store: &Store,
        _reporter: &HookInstallReporter,
    ) -> Result<InstalledHook> {
        Ok(InstalledHook::NoNeedInstall(hook))
    }

    async fn check_health(&self, _env: &InstalledHookEnv) -> Result<()> {
        Ok(())
    }

    async fn run(
        &self,
        hook: &InstalledHook,
        _filenames: &[&Path],
        _store: &Store,
        _reporter: &HookRunReporter,
    ) -> Result<(i32, Vec<u8>)> {
        anyhow::bail!(UnimplementedError(format!("{}", hook.language)))
    }
}

// `pre-commit` language support:
// bun: install requested version, support env, support additional deps
// conda: only system version, support env, support additional deps
// coursier: only system version, support env, support additional deps
// dart: only system version, support env, support additional deps
// docker_image: only system version, no env, no additional deps
// docker: only system version, support env, no additional deps
// dotnet: install requested version, support env, support additional deps
// fail: only system version, no env, no additional deps
// golang: install requested version, support env, support additional deps
// haskell: only system version, support env, support additional deps
// lua: only system version, support env, support additional deps
// node: install requested version, support env, support additional deps (delegated to nodeenv)
// perl: only system version, support env, support additional deps
// pygrep: only system version, no env, no additional deps
// python: install requested version, support env, support additional deps (delegated to virtualenv)
// python_uv: install requested version, support env, no additional deps
// r: only system version, support env, support additional deps
// ruby: install requested version, support env, support additional deps (delegated to rbenv)
// rust: install requested version, support env, support additional deps (delegated to rustup and cargo)
// script: only system version, no env, no additional deps
// swift: only system version, support env, no additional deps
// system: only system version, no env, no additional deps

impl Language {
    pub(crate) fn supported(lang: Language) -> bool {
        match lang {
            Self::Bun
            | Self::Dart
            | Self::Deno
            | Self::Docker
            | Self::DockerImage
            | Self::Dotnet
            | Self::Fail
            | Self::Golang
            | Self::Haskell
            | Self::Julia
            | Self::Lua
            | Self::Node
            | Self::Pygrep
            | Self::Python
            | Self::PythonUv
            | Self::Ruby
            | Self::Rust
            | Self::Script
            | Self::Swift
            | Self::System => true,
            Self::Conda | Self::Coursier | Self::Perl | Self::R => false,
        }
    }

    pub(crate) fn supports_install_env(self) -> bool {
        match self {
            Self::Bun
            | Self::Conda
            | Self::Coursier
            | Self::Dart
            | Self::Deno
            | Self::Docker
            | Self::Dotnet
            | Self::Golang
            | Self::Haskell
            | Self::Julia
            | Self::Lua
            | Self::Node
            | Self::Perl
            | Self::Pygrep
            | Self::Python
            | Self::PythonUv
            | Self::R
            | Self::Ruby
            | Self::Rust
            | Self::Swift => true,
            Self::DockerImage | Self::Fail | Self::Script | Self::System => false,
        }
    }

    pub(crate) fn shell_support(self) -> ShellSupport {
        match self {
            Self::Bun
            | Self::Deno
            | Self::Dotnet
            | Self::Golang
            | Self::Haskell
            | Self::Lua
            | Self::Node
            | Self::Python
            | Self::PythonUv
            | Self::Ruby
            | Self::Script
            | Self::Swift
            | Self::System => ShellSupport::Supported,
            Self::Conda | Self::Coursier | Self::Perl | Self::R => {
                ShellSupport::Unsupported("no runner is implemented yet")
            }
            Self::Dart => ShellSupport::Unsupported(
                "`--packages` injection requires the resolved argv to contain `dart` directly",
            ),
            Self::Docker | Self::DockerImage => ShellSupport::Unsupported(
                "`entry` participates in container image or entrypoint selection",
            ),
            Self::Fail => ShellSupport::Unsupported("`entry` is the failure message body"),
            Self::Julia | Self::Rust => ShellSupport::Unsupported(
                "`entry` participates in install/runtime package resolution and is split before execution",
            ),
            Self::Pygrep => ShellSupport::Unsupported("`entry` is the regex pattern"),
        }
    }

    pub(crate) fn tool_buckets(self) -> &'static [ToolBucket] {
        match self {
            Self::Bun => &[ToolBucket::Bun],
            Self::Deno => &[ToolBucket::Deno],
            Self::Dotnet => &[ToolBucket::Dotnet],
            Self::Golang => &[ToolBucket::Go],
            Self::Node => &[ToolBucket::Node],
            Self::Python | Self::PythonUv | Self::Pygrep => &[ToolBucket::Uv, ToolBucket::Python],
            Self::Ruby => &[ToolBucket::Ruby],
            Self::Rust => &[ToolBucket::Rustup],
            Self::Conda
            | Self::Coursier
            | Self::Dart
            | Self::Docker
            | Self::DockerImage
            | Self::Fail
            | Self::Haskell
            | Self::Julia
            | Self::Lua
            | Self::Perl
            | Self::R
            | Self::Script
            | Self::Swift
            | Self::System => &[],
        }
    }

    pub(crate) fn cache_buckets(self) -> &'static [CacheBucket] {
        match self {
            Self::Deno => &[CacheBucket::Deno],
            Self::Golang => &[CacheBucket::Go],
            Self::Python | Self::PythonUv | Self::Pygrep => &[CacheBucket::Uv, CacheBucket::Python],
            Self::Rust => &[CacheBucket::Cargo],
            Self::Bun
            | Self::Conda
            | Self::Coursier
            | Self::Dart
            | Self::Docker
            | Self::DockerImage
            | Self::Dotnet
            | Self::Fail
            | Self::Haskell
            | Self::Julia
            | Self::Lua
            | Self::Node
            | Self::Perl
            | Self::R
            | Self::Ruby
            | Self::Script
            | Self::Swift
            | Self::System => &[],
        }
    }

    /// Return whether the language allows specifying the version, e.g. we can install a specific
    /// requested language version.
    /// See <https://pre-commit.com/#overriding-language-version>
    pub(crate) fn supports_language_version(self) -> bool {
        match self {
            Self::Bun
            | Self::Deno
            | Self::Dotnet
            | Self::Golang
            | Self::Node
            | Self::Python
            | Self::PythonUv
            | Self::Ruby
            | Self::Rust => true,
            Self::Conda
            | Self::Coursier
            | Self::Dart
            | Self::Docker
            | Self::DockerImage
            | Self::Fail
            | Self::Haskell
            | Self::Julia
            | Self::Lua
            | Self::Perl
            | Self::Pygrep
            | Self::R
            | Self::Script
            | Self::Swift
            | Self::System => false,
        }
    }

    /// Whether the language supports installing dependencies.
    ///
    /// For example, Python and Node.js support installing dependencies, while
    /// System and Fail do not.
    pub(crate) fn supports_dependency(self) -> bool {
        match self {
            Self::Bun
            | Self::Conda
            | Self::Coursier
            | Self::Dart
            | Self::Deno
            | Self::Dotnet
            | Self::Golang
            | Self::Haskell
            | Self::Julia
            | Self::Lua
            | Self::Node
            | Self::Perl
            | Self::Python
            | Self::R
            | Self::Ruby
            | Self::Rust => true,
            Self::Docker
            | Self::DockerImage
            | Self::Fail
            | Self::Pygrep
            | Self::PythonUv
            | Self::Script
            | Self::Swift
            | Self::System => false,
        }
    }

    pub(crate) async fn install(
        &self,
        hook: Arc<Hook>,
        store: &Store,
        reporter: &HookInstallReporter,
    ) -> Result<InstalledHook> {
        match self {
            Self::Dart => dart::Dart.install(hook, store, reporter).await,
            Self::Bun => bun::Bun.install(hook, store, reporter).await,
            Self::Deno => deno::Deno.install(hook, store, reporter).await,
            Self::Docker => docker::Docker.install(hook, store, reporter).await,
            Self::DockerImage => {
                docker_image::DockerImage
                    .install(hook, store, reporter)
                    .await
            }
            Self::Dotnet => dotnet::Dotnet.install(hook, store, reporter).await,
            Self::Fail => fail::Fail.install(hook, store, reporter).await,
            Self::Golang => golang::Golang.install(hook, store, reporter).await,
            Self::Haskell => haskell::Haskell.install(hook, store, reporter).await,
            Self::Julia => julia::Julia.install(hook, store, reporter).await,
            Self::Lua => lua::Lua.install(hook, store, reporter).await,
            Self::Node => node::Node.install(hook, store, reporter).await,
            Self::Pygrep => pygrep::Pygrep.install(hook, store, reporter).await,
            Self::Python => python::Python.install(hook, store, reporter).await,
            Self::PythonUv => python::PythonUv.install(hook, store, reporter).await,
            Self::Ruby => ruby::Ruby.install(hook, store, reporter).await,
            Self::Rust => rust::Rust.install(hook, store, reporter).await,
            Self::Script => script::Script.install(hook, store, reporter).await,
            Self::Swift => swift::Swift.install(hook, store, reporter).await,
            Self::System => system::System.install(hook, store, reporter).await,
            Self::Conda | Self::Coursier | Self::Perl | Self::R => {
                Unimplemented.install(hook, store, reporter).await
            }
        }
    }

    pub(crate) async fn check_health(&self, env: &InstalledHookEnv) -> Result<()> {
        match self {
            Self::Dart => dart::Dart.check_health(env).await,
            Self::Bun => bun::Bun.check_health(env).await,
            Self::Deno => deno::Deno.check_health(env).await,
            Self::Docker => docker::Docker.check_health(env).await,
            Self::DockerImage => docker_image::DockerImage.check_health(env).await,
            Self::Dotnet => dotnet::Dotnet.check_health(env).await,
            Self::Fail => fail::Fail.check_health(env).await,
            Self::Golang => golang::Golang.check_health(env).await,
            Self::Haskell => haskell::Haskell.check_health(env).await,
            Self::Julia => julia::Julia.check_health(env).await,
            Self::Lua => lua::Lua.check_health(env).await,
            Self::Node => node::Node.check_health(env).await,
            Self::Pygrep => pygrep::Pygrep.check_health(env).await,
            Self::Python => python::Python.check_health(env).await,
            Self::PythonUv => python::PythonUv.check_health(env).await,
            Self::Ruby => ruby::Ruby.check_health(env).await,
            Self::Rust => rust::Rust.check_health(env).await,
            Self::Script => script::Script.check_health(env).await,
            Self::Swift => swift::Swift.check_health(env).await,
            Self::System => system::System.check_health(env).await,
            Self::Conda | Self::Coursier | Self::Perl | Self::R => {
                Unimplemented.check_health(env).await
            }
        }
    }

    #[instrument(level = "trace", skip_all, fields(hook_id = %hook.id, language = %hook.language))]
    pub(crate) async fn run(
        &self,
        hook: &InstalledHook,
        filenames: &[&Path],
        store: &Store,
        reporter: &HookRunReporter,
    ) -> Result<(i32, Vec<u8>)> {
        match hook.repo() {
            Repo::Meta { .. } => {
                return hooks::MetaHooks::from_str(&hook.id)
                    .unwrap()
                    .run(store, hook, filenames, reporter)
                    .await;
            }
            Repo::Builtin { .. } => {
                return hooks::BuiltinHooks::from_str(&hook.id)
                    .unwrap()
                    .run(store, hook, filenames, reporter)
                    .await;
            }
            Repo::Remote { .. } => {
                // Fast path for hooks implemented in Rust
                if hooks::check_fast_path(hook) {
                    return hooks::run_fast_path(store, hook, filenames, reporter).await;
                }
            }
            Repo::Local { .. } => {}
        }

        match self {
            Self::Dart => dart::Dart.run(hook, filenames, store, reporter).await,
            Self::Bun => bun::Bun.run(hook, filenames, store, reporter).await,
            Self::Deno => deno::Deno.run(hook, filenames, store, reporter).await,
            Self::Docker => docker::Docker.run(hook, filenames, store, reporter).await,
            Self::DockerImage => {
                docker_image::DockerImage
                    .run(hook, filenames, store, reporter)
                    .await
            }
            Self::Dotnet => dotnet::Dotnet.run(hook, filenames, store, reporter).await,
            Self::Fail => fail::Fail.run(hook, filenames, store, reporter).await,
            Self::Golang => golang::Golang.run(hook, filenames, store, reporter).await,
            Self::Haskell => haskell::Haskell.run(hook, filenames, store, reporter).await,
            Self::Julia => julia::Julia.run(hook, filenames, store, reporter).await,
            Self::Lua => lua::Lua.run(hook, filenames, store, reporter).await,
            Self::Node => node::Node.run(hook, filenames, store, reporter).await,
            Self::Pygrep => pygrep::Pygrep.run(hook, filenames, store, reporter).await,
            Self::Python => python::Python.run(hook, filenames, store, reporter).await,
            Self::PythonUv => python::PythonUv.run(hook, filenames, store, reporter).await,
            Self::Ruby => ruby::Ruby.run(hook, filenames, store, reporter).await,
            Self::Rust => rust::Rust.run(hook, filenames, store, reporter).await,
            Self::Script => script::Script.run(hook, filenames, store, reporter).await,
            Self::Swift => swift::Swift.run(hook, filenames, store, reporter).await,
            Self::System => system::System.run(hook, filenames, store, reporter).await,
            Self::Conda | Self::Coursier | Self::Perl | Self::R => {
                Unimplemented.run(hook, filenames, store, reporter).await
            }
        }
    }
}

/// Try to extract metadata before the hook installation identity is finalized.
pub(crate) async fn extract_metadata(metadata: &mut HookMetadata<'_>) -> Result<()> {
    match metadata.language {
        Language::Python => python::extract_metadata(metadata).await,
        Language::Golang => golang::extract_go_mod_metadata(metadata).await,
        Language::Bun
        | Language::Conda
        | Language::Coursier
        | Language::Dart
        | Language::Deno
        | Language::Docker
        | Language::DockerImage
        | Language::Dotnet
        | Language::Fail
        | Language::Haskell
        | Language::Julia
        | Language::Lua
        | Language::Node
        | Language::Perl
        | Language::Pygrep
        | Language::PythonUv
        | Language::R
        | Language::Ruby
        | Language::Rust
        | Language::Script
        | Language::Swift
        | Language::System => Ok(()),
    }
}

/// Resolve the actual process invocation, honoring shebangs and PATH lookups.
pub(crate) fn resolve_command(mut cmds: Vec<String>, paths: Option<&OsStr>) -> Vec<String> {
    let env_path = if paths.is_none() {
        EnvVars::var_os(EnvVars::PATH)
    } else {
        None
    };
    let paths = paths.or(env_path.as_deref());

    let candidate = &cmds[0];
    let resolved_binary = match which::which_in(candidate, paths, &*CWD) {
        Ok(p) => p,
        Err(_) => PathBuf::from(candidate),
    };
    trace!("Resolved command: {}", resolved_binary.display());

    if let Ok(mut shebang_argv) = parse_shebang(&resolved_binary) {
        trace!("Found shebang: {:?}", shebang_argv);
        #[allow(unused_mut)]
        let mut interpreter = shebang_argv[0].as_str();
        #[cfg(windows)]
        {
            let interpreter_path = Path::new(interpreter);
            // Git for Windows behavior: if a shebang points to a Unix-style absolute
            // interpreter path (e.g. `/bin/sh`) that does not exist on Windows,
            // fall back to PATH lookup of its basename (`sh`).
            if !interpreter_path.exists()
                // Restrict this fallback to path-like interpreter values so plain
                // commands (like `python`) keep their normal resolution path below.
                && (interpreter_path.has_root() || interpreter.contains(['/', '\\']))
                // Extract basename from shebang path (`/bin/sh` -> `sh`) and resolve it.
                && let Some(file_name) = interpreter_path.file_name().and_then(OsStr::to_str)
            {
                interpreter = file_name;
            }
        }
        // Resolve the interpreter path, convert "python3" to "python3.exe" on Windows
        if let Ok(p) = which::which_in(interpreter, paths, &*CWD) {
            shebang_argv[0] = p.to_string_lossy().to_string();
            trace!("Resolved interpreter: {}", shebang_argv[0]);
        }
        shebang_argv.push(resolved_binary.to_string_lossy().to_string());
        shebang_argv.extend_from_slice(&cmds[1..]);
        shebang_argv
    } else {
        cmds[0] = resolved_binary.to_string_lossy().to_string();
        cmds
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::path::Path;

    use tempfile::tempdir;

    use super::resolve_command;

    fn write_file(path: &Path, contents: &str) {
        fs_err::write(path, contents).expect("write test file");
    }

    #[cfg(unix)]
    fn make_executable(path: &Path) {
        use std::os::unix::fs::PermissionsExt;

        let metadata = fs_err::metadata(path).expect("stat test file");
        let mut perms = metadata.permissions();
        perms.set_mode(perms.mode() | 0o111);
        fs_err::set_permissions(path, perms).expect("set executable bit");
    }

    #[cfg(windows)]
    fn make_executable(_path: &Path) {}

    #[test]
    fn resolve_command_passthrough_when_not_found() {
        let cmd = "__prek_nonexistent_command__".to_string();
        let resolved = resolve_command(vec![cmd.clone()], None);
        assert_eq!(resolved, vec![cmd]);
    }

    #[test]
    fn resolve_command_resolves_shebang_interpreter_from_path() {
        let dir = tempdir().expect("create temp dir");
        let script_path = dir.path().join("hook-script");
        write_file(
            &script_path,
            "#!/usr/bin/env prek-test-interpreter\necho hi\n",
        );

        #[cfg(windows)]
        let interpreter_path = dir.path().join("prek-test-interpreter.exe");
        #[cfg(not(windows))]
        let interpreter_path = dir.path().join("prek-test-interpreter");

        write_file(&interpreter_path, "");
        make_executable(&interpreter_path);

        let paths = OsString::from(dir.path().as_os_str());
        let resolved = resolve_command(
            vec![script_path.to_string_lossy().into_owned()],
            Some(paths.as_os_str()),
        );

        assert_eq!(resolved[0], interpreter_path.to_string_lossy());
        assert_eq!(resolved[1], script_path.to_string_lossy());
    }

    #[cfg(windows)]
    #[test]
    fn resolve_command_windows_rewrites_bin_sh_to_path_sh() {
        let dir = tempdir().expect("create temp dir");
        let script_path = dir.path().join("legacy-hook");
        write_file(&script_path, "#!/bin/sh\necho legacy\n");

        let sh_path = dir.path().join("sh.exe");
        write_file(&sh_path, "");

        let paths = OsString::from(dir.path().as_os_str());
        let resolved = resolve_command(
            vec![script_path.to_string_lossy().into_owned()],
            Some(paths.as_os_str()),
        );

        assert_eq!(resolved[0], sh_path.to_string_lossy());
        assert_eq!(resolved[1], script_path.to_string_lossy());
    }

    #[cfg(windows)]
    #[test]
    fn resolve_command_windows_keeps_existing_absolute_interpreter_path() {
        let dir = tempdir().expect("create temp dir");

        let interp_dir = dir.path().join("bin");
        fs_err::create_dir_all(&interp_dir).expect("create interpreter dir");
        let interp_path = interp_dir.join("sh.exe");
        write_file(&interp_path, "");
        let shebang_interpreter = interp_path.to_string_lossy().replace('\\', "/");

        let script_path = dir.path().join("legacy-hook");
        write_file(
            &script_path,
            &format!("#!{shebang_interpreter}\necho legacy\n"),
        );

        let paths = OsString::from(dir.path().as_os_str());
        let resolved = resolve_command(
            vec![script_path.to_string_lossy().into_owned()],
            Some(paths.as_os_str()),
        );

        let resolved_interp = Path::new(&resolved[0]);
        assert_eq!(resolved_interp, interp_path.as_path());
        assert_eq!(resolved[1], script_path.to_string_lossy());
    }
}
