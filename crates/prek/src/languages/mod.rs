use std::ffi::{OsStr, OsString};
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::process::Stdio;
use std::str::FromStr;
use std::sync::Arc;

use anyhow::{Context, Result};
use prek_consts::env_vars::{EnvVars, EnvVarsRead};
use prek_identify::parse_shebang;
use tracing::{instrument, trace};

use crate::cli::reporter::HookInstallReporter;
use crate::cli::run::HookRunReporter;
use crate::config::Language;
use crate::fs::PathClean;
use crate::hook::{Hook, InstallInfo, InstalledHook, Repo};
use crate::hook_entry::PreparedHookEntry;
use crate::hooks::{self, HookOutput};
use crate::process::Cmd;
use crate::run::run_by_batch;
use crate::store::{CacheBucket, Store, ToolBucket};

mod bun;
mod conda;
mod coursier;
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
mod mise;
mod node;
mod perl;
mod php;
mod pygrep;
mod python;
mod r;
mod ruby;
mod rust;
mod script;
mod swift;
mod system;
pub(crate) mod version;

// Backend futures are awaited in place rather than spawned. Requiring `Send` here would impose a
// stronger contract than callers need and rejects the borrowed async closures used by backends.
#[async_trait::async_trait(?Send)]
trait LanguageBackend: Sync {
    /// Provisions the environment required by `hook` and returns the prepared hook.
    async fn install(
        &self,
        store: &Store,
        hook: Arc<Hook>,
        reporter: &HookInstallReporter,
    ) -> Result<InstalledHook>;

    /// Checks whether the installed environment described by `info` can be reused.
    async fn check_health(&self, info: &InstallInfo) -> Result<()>;

    /// Builds the language-specific base environment for hook commands.
    ///
    /// The default leaves the caller's environment unchanged.
    fn execution_environment(
        &self,
        _store: &Store,
        _hook: &InstalledHook,
    ) -> Result<ExecutionEnvironment> {
        Ok(ExecutionEnvironment::default())
    }

    /// Resolves the configured entry after the execution environment is prepared.
    ///
    /// This is used for normal hook runs; `prek exec` executes its supplied command directly.
    fn prepare_hook_entry(
        &self,
        store: &Store,
        hook: &InstalledHook,
        environment: &ExecutionEnvironment,
    ) -> Result<PreparedHookEntry> {
        Ok(hook
            .entry
            .resolve(environment.path(hook), hook.work_dir(), store)?)
    }

    /// Applies asynchronous or working-directory-dependent changes to `environment`.
    ///
    /// This is called after [`Self::execution_environment`] and before command resolution. `cwd`
    /// is the hook work directory for a normal run and the caller's working directory for
    /// `prek exec`.
    async fn prepare_execution_environment(
        &self,
        _hook: &InstalledHook,
        _cwd: &Path,
        _environment: &mut ExecutionEnvironment,
    ) -> Result<()> {
        Ok(())
    }

    /// Runs the hook for `filenames`, reports progress, and returns its exit code and output.
    ///
    /// The default prepares the environment, resolves the entry, and executes filename batches
    /// from the hook work directory.
    async fn run(
        &self,
        store: &Store,
        hook: &InstalledHook,
        filenames: &[&Path],
        reporter: &HookRunReporter,
    ) -> Result<(i32, Vec<u8>)> {
        let progress = reporter.on_run_start(hook, filenames.len());

        let mut environment = self.execution_environment(store, hook)?;
        self.prepare_execution_environment(hook, hook.work_dir(), &mut environment)
            .await?;
        let entry = self.prepare_hook_entry(store, hook, &environment)?;
        let run = async |batch: &[&Path]| {
            let output = environment
                .command(hook, hook.work_dir(), entry.argv())?
                .args(&hook.args)
                .file_args(batch)
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

/// Process environment shared by normal hook runs and `prek exec`.
///
/// Language defaults are applied before the hook's configured `env`.
#[derive(Debug, Default)]
pub(crate) struct ExecutionEnvironment {
    path: Option<OsString>,
    patch: EnvironmentPatch,
}

#[derive(Debug, Default)]
struct EnvironmentPatch {
    set: Vec<(OsString, OsString)>,
    remove: Vec<OsString>,
}

impl EnvironmentPatch {
    fn set(&mut self, key: impl AsRef<OsStr>, value: impl AsRef<OsStr>) {
        self.set
            .push((key.as_ref().to_os_string(), value.as_ref().to_os_string()));
    }

    fn remove(&mut self, key: impl AsRef<OsStr>) {
        self.remove.push(key.as_ref().to_os_string());
    }

    fn apply(&self, cmd: &mut Cmd) {
        for key in &self.remove {
            cmd.env_remove(key);
        }
        cmd.envs(self.set.iter().map(|(key, value)| (key, value)));
    }
}

impl ExecutionEnvironment {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn set_path(&mut self, path: impl AsRef<OsStr>) -> &mut Self {
        let path = path.as_ref().to_os_string();
        self.patch.set(EnvVars::PATH, &path);
        self.path = Some(path);
        self
    }

    pub(crate) fn env(&mut self, key: impl AsRef<OsStr>, value: impl AsRef<OsStr>) -> &mut Self {
        self.patch.set(key, value);
        self
    }

    pub(crate) fn envs<I, K, V>(&mut self, vars: I) -> &mut Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<OsStr>,
        V: AsRef<OsStr>,
    {
        for (key, value) in vars {
            self.env(key, value);
        }
        self
    }

    pub(crate) fn env_remove(&mut self, key: impl AsRef<OsStr>) -> &mut Self {
        self.patch.remove(key);
        self
    }

    pub(crate) fn command(
        &self,
        hook: &InstalledHook,
        cwd: &Path,
        command: &[OsString],
    ) -> Result<Cmd> {
        let command = resolve_command(command.to_vec(), self.path(hook), cwd);
        let (program, args) = command.split_first().context("Command cannot be empty")?;
        let mut cmd = Cmd::new(program);
        cmd.current_dir(cwd).args(args).check(false);
        self.patch.apply(&mut cmd);
        cmd.envs(&hook.env);
        Ok(cmd)
    }

    /// Effective PATH after applying the hook's configured environment.
    pub(crate) fn path<'a>(&'a self, hook: &'a InstalledHook) -> Option<&'a OsStr> {
        hook.env
            .iter()
            .find_map(|(key, value)| {
                if is_path_env(key) {
                    Some(OsStr::new(value))
                } else {
                    None
                }
            })
            .or(self.language_path())
    }

    /// PATH supplied by the language backend before hook-specific overrides.
    fn language_path(&self) -> Option<&OsStr> {
        self.path.as_deref()
    }
}

fn is_path_env(key: impl AsRef<OsStr>) -> bool {
    let key = key.as_ref();
    #[cfg(windows)]
    {
        key.to_string_lossy().eq_ignore_ascii_case(EnvVars::PATH)
    }
    #[cfg(not(windows))]
    {
        key == OsStr::new(EnvVars::PATH)
    }
}

type LanguageFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T>> + 'a>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShellSupport {
    Supported,
    Unsupported(&'static str),
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
// mise: install requested version, support env, support additional deps
// node: install requested version, support env, support additional deps (delegated to nodeenv)
// perl: only system version, support env, support additional deps
// php: only system version, support env, support additional deps
// pygrep: only system version, no env, no additional deps
// python: install requested version, support env, support additional deps (delegated to virtualenv)
// r: only system version, support env, support additional deps
// ruby: install requested version, support env, support additional deps (delegated to rbenv)
// rust: install requested version, support env, support additional deps (delegated to rustup and cargo)
// script: only system version, no env, no additional deps
// swift: only system version, support env, no additional deps
// system: only system version, no env, no additional deps

impl Language {
    fn backend(self) -> &'static dyn LanguageBackend {
        match self {
            Self::Bun => &bun::Bun,
            Self::Conda => &conda::Conda,
            Self::Coursier => &coursier::Coursier,
            Self::Dart => &dart::Dart,
            Self::Deno => &deno::Deno,
            Self::Docker => &docker::Docker,
            Self::DockerImage => &docker_image::DockerImage,
            Self::Dotnet => &dotnet::Dotnet,
            Self::Fail => &fail::Fail,
            Self::Golang => &golang::Golang,
            Self::Haskell => &haskell::Haskell,
            Self::Julia => &julia::Julia,
            Self::Lua => &lua::Lua,
            Self::Mise => &mise::Mise,
            Self::Node => &node::Node,
            Self::Perl => &perl::Perl,
            Self::Php => &php::Php,
            Self::Pygrep => &pygrep::Pygrep,
            Self::Python => &python::Python,
            Self::R => &r::R,
            Self::Ruby => &ruby::Ruby,
            Self::Rust => &rust::Rust,
            Self::Script => &script::Script,
            Self::Swift => &swift::Swift,
            Self::System => &system::System,
        }
    }

    pub(crate) fn can_modify_files(self) -> bool {
        !matches!(self, Self::Fail | Self::Pygrep)
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
            | Self::Mise
            | Self::Node
            | Self::Perl
            | Self::Php
            | Self::Pygrep
            | Self::Python
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
            | Self::Conda
            | Self::Coursier
            | Self::Deno
            | Self::Dotnet
            | Self::Golang
            | Self::Haskell
            | Self::Lua
            | Self::Mise
            | Self::Node
            | Self::Perl
            | Self::Php
            | Self::Python
            | Self::Ruby
            | Self::Script
            | Self::Swift
            | Self::System => ShellSupport::Supported,
            Self::R => ShellSupport::Unsupported(
                "`entry` must use the R backend's `Rscript -e <expr>` or `Rscript <file>` forms",
            ),
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
            Self::Mise => &[ToolBucket::Mise],
            Self::Node => &[ToolBucket::Node],
            Self::Python | Self::Pygrep => &[ToolBucket::Uv, ToolBucket::Python],
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
            | Self::Php
            | Self::R
            | Self::Script
            | Self::Swift
            | Self::System => &[],
        }
    }

    pub(crate) fn cache_buckets(self) -> &'static [CacheBucket] {
        match self {
            Self::Coursier => &[CacheBucket::Coursier],
            Self::Deno => &[CacheBucket::Deno],
            Self::Golang => &[CacheBucket::Go],
            Self::Node => &[CacheBucket::Npm],
            Self::Python | Self::Pygrep => &[CacheBucket::Uv, CacheBucket::Python],
            Self::Rust => &[CacheBucket::Cargo],
            Self::Bun
            | Self::Conda
            | Self::Dart
            | Self::Docker
            | Self::DockerImage
            | Self::Dotnet
            | Self::Fail
            | Self::Haskell
            | Self::Julia
            | Self::Lua
            | Self::Mise
            | Self::Perl
            | Self::Php
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
            | Self::Mise
            | Self::Node
            | Self::Python
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
            | Self::Php
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
            | Self::Mise
            | Self::Node
            | Self::Perl
            | Self::Php
            | Self::Python
            | Self::R
            | Self::Ruby
            | Self::Rust => true,
            Self::Docker
            | Self::DockerImage
            | Self::Fail
            | Self::Pygrep
            | Self::Script
            | Self::Swift
            | Self::System => false,
        }
    }

    pub(crate) fn ensure_exec_supported(self, hook: &Hook) -> Result<()> {
        match hook.repo() {
            Repo::Meta { .. } => anyhow::bail!("`prek exec` does not support meta hooks"),
            Repo::Builtin { .. } => anyhow::bail!("`prek exec` does not support builtin hooks"),
            Repo::Remote { .. } | Repo::Local { .. } => {}
        }

        match self {
            Self::Bun
            | Self::Conda
            | Self::Coursier
            | Self::Dart
            | Self::Deno
            | Self::Dotnet
            | Self::Golang
            | Self::Haskell
            | Self::Lua
            | Self::Mise
            | Self::Node
            | Self::Perl
            | Self::Php
            | Self::Python
            | Self::R
            | Self::Ruby
            | Self::Rust
            | Self::Script
            | Self::Swift
            | Self::System => Ok(()),
            Self::Docker | Self::DockerImage | Self::Fail | Self::Julia | Self::Pygrep => {
                anyhow::bail!("`prek exec` does not support hooks with language `{self}`")
            }
        }
    }

    pub(crate) fn install<'a>(
        &'a self,
        store: &'a Store,
        hook: Arc<Hook>,
        reporter: &'a HookInstallReporter,
    ) -> LanguageFuture<'a, InstalledHook> {
        self.backend().install(store, hook, reporter)
    }

    pub(crate) fn check_health<'a>(&'a self, info: &'a InstallInfo) -> LanguageFuture<'a, ()> {
        self.backend().check_health(info)
    }

    #[instrument(skip_all, fields(hook_id=%hook.id, language=%hook.language))]
    pub(crate) async fn run<'a, 'p>(
        &'a self,
        store: &'a Store,
        hook: &'a InstalledHook,
        filenames: &'a [&'p Path],
        reporter: &'a HookRunReporter,
    ) -> Result<HookOutput>
    where
        'p: 'a,
    {
        match hook.repo() {
            Repo::Meta { .. } => {
                hooks::MetaHooks::from_str(&hook.id)
                    .unwrap()
                    .run(store, hook, filenames, reporter)
                    .await
            }
            Repo::Builtin { .. } => {
                hooks::BuiltinHooks::from_str(&hook.id)
                    .unwrap()
                    .run(store, hook, filenames, reporter)
                    .await
            }
            // Fast path for hooks implemented in Rust
            Repo::Remote { .. } if hooks::check_fast_path(hook) => {
                hooks::run_fast_path(store, hook, filenames, reporter).await
            }
            Repo::Remote { .. } | Repo::Local { .. } => {
                let (exit_status, output) =
                    self.backend().run(store, hook, filenames, reporter).await?;
                if self.can_modify_files() {
                    Ok(HookOutput::unknown(exit_status, output))
                } else {
                    Ok(HookOutput::unchanged(exit_status, output))
                }
            }
        }
    }

    #[instrument(skip_all, fields(hook_id=%hook.id, language=%hook.language))]
    pub(crate) async fn exec(
        &self,
        store: &Store,
        hook: &InstalledHook,
        cwd: &Path,
        command: &[OsString],
    ) -> Result<std::process::ExitStatus> {
        self.ensure_exec_supported(hook)?;

        let mut environment = self.backend().execution_environment(store, hook)?;
        self.backend()
            .prepare_execution_environment(hook, cwd, &mut environment)
            .await?;
        environment
            .command(hook, cwd, command)?
            .status()
            .await
            .map_err(Into::into)
    }
}

/// Try to extract metadata from the given hook.
pub(crate) async fn extract_metadata(hook: &mut Hook) -> Result<()> {
    match hook.language {
        Language::Python => python::extract_metadata(hook).await,
        Language::Golang => golang::extract_go_mod_metadata(hook).await,
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
        | Language::Mise
        | Language::Node
        | Language::Perl
        | Language::Php
        | Language::Pygrep
        | Language::R
        | Language::Ruby
        | Language::Rust
        | Language::Script
        | Language::Swift
        | Language::System => Ok(()),
    }
}

/// Resolve the actual process invocation, honoring shebangs and PATH lookups.
pub(crate) fn resolve_command(
    mut cmds: Vec<OsString>,
    paths: Option<&OsStr>,
    cwd: &Path,
) -> Vec<OsString> {
    let Some(candidate) = cmds.first() else {
        return cmds;
    };

    let env_path = if paths.is_none() {
        EnvVars.var_os(EnvVars::PATH)
    } else {
        None
    };
    let paths = paths.or(env_path.as_deref());

    let resolved_binary = which::which_in(candidate, paths, cwd).unwrap_or_else(|_| {
        let candidate = Path::new(candidate);
        let has_parent = candidate
            .parent()
            .is_some_and(|parent| !parent.as_os_str().is_empty());
        // `which_in` only returns executable files. Resolve an explicit relative
        // path against `cwd` so its shebang can still be inspected.
        if candidate.is_absolute() || has_parent {
            cwd.join(candidate).clean()
        } else {
            candidate.into()
        }
    });

    let Ok(shebang_argv) = parse_shebang(&resolved_binary) else {
        cmds[0] = resolved_binary.into_os_string();
        return cmds;
    };

    let mut shebang_argv = shebang_argv
        .into_iter()
        .map(OsString::from)
        .collect::<Vec<_>>();
    trace!("Found shebang: {:?}", shebang_argv);
    let interpreter = shebang_argv[0].as_os_str();
    #[cfg(windows)]
    let interpreter = {
        let interpreter_path = Path::new(interpreter);
        if !interpreter_path.exists()
            && (interpreter_path.has_root() || interpreter_path.components().count() > 1)
        {
            // Git for Windows behavior: if a shebang points to a Unix-style absolute
            // interpreter path (e.g. `/bin/sh`) that does not exist on Windows,
            // fall back to PATH lookup of its basename (`sh`).
            interpreter_path.file_name().unwrap_or(interpreter)
        } else {
            interpreter
        }
    };

    // Resolve the interpreter path, converting "python3" to "python3.exe" on Windows.
    if let Ok(path) = which::which_in(interpreter, paths, cwd) {
        shebang_argv[0] = path.into_os_string();
    }
    shebang_argv.push(resolved_binary.into_os_string());
    shebang_argv.extend(cmds.drain(1..));
    shebang_argv
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use std::ffi::OsStr;
    use std::ffi::OsString;
    #[cfg(unix)]
    use std::os::unix::ffi::OsStringExt;
    use std::path::Path;

    use tempfile::tempdir;

    use super::resolve_command;
    use crate::fs::{CWD, PathClean, make_executable};

    fn write_file(path: &Path, contents: &str) {
        fs_err::write(path, contents).expect("write test file");
    }

    #[test]
    fn resolve_command_passthrough_when_not_found() {
        let cmd = OsString::from("__prek_nonexistent_command__");
        let resolved = resolve_command(vec![cmd.clone()], None, &CWD);
        assert_eq!(resolved, vec![cmd]);
    }

    #[test]
    fn resolve_command_resolves_missing_relative_path_from_cwd() {
        let cmd = OsString::from("./__prek_nonexistent_command__");
        let paths = OsString::new();
        let resolved = resolve_command(vec![cmd.clone()], Some(paths.as_os_str()), &CWD);
        assert_eq!(resolved, vec![CWD.join(cmd).clean().into_os_string()]);
    }

    #[test]
    fn resolve_command_passthrough_when_empty() {
        assert_eq!(
            resolve_command(Vec::new(), None, &CWD),
            Vec::<OsString>::new()
        );
    }

    #[cfg(unix)]
    #[test]
    fn resolve_command_preserves_non_utf8_arguments() {
        let cmd = OsString::from("__prek_nonexistent_command__");
        let arg = OsString::from_vec(vec![b'f', b'o', 0x80]);

        assert_eq!(
            resolve_command(vec![cmd.clone(), arg.clone()], Some(OsStr::new("")), &CWD,),
            vec![cmd, arg]
        );
    }

    #[test]
    fn resolve_command_resolves_shebang_and_preserves_arguments() {
        let dir = tempdir().expect("create temp dir");
        let script_path = dir.path().join("hook-script");
        write_file(
            &script_path,
            "#!/usr/bin/env -S prek-test-interpreter --from-shebang\necho hi\n",
        );

        #[cfg(windows)]
        let interpreter_path = dir.path().join("prek-test-interpreter.exe");
        #[cfg(not(windows))]
        let interpreter_path = dir.path().join("prek-test-interpreter");

        write_file(&interpreter_path, "");
        make_executable(&interpreter_path).expect("set executable bit");

        let paths = OsString::from(dir.path().as_os_str());
        let script = script_path.into_os_string();
        let resolved = resolve_command(
            vec![script.clone(), OsString::from("--from-entry")],
            Some(paths.as_os_str()),
            &CWD,
        );

        assert_eq!(
            resolved,
            vec![
                interpreter_path.into_os_string(),
                OsString::from("--from-shebang"),
                script,
                OsString::from("--from-entry"),
            ]
        );
    }

    #[test]
    fn resolve_command_resolves_relative_shebang_from_cwd() {
        let dir = tempdir().expect("create temp dir");
        let script_path = dir.path().join("hook-script");
        write_file(
            &script_path,
            "#!/usr/bin/env -S prek-test-interpreter --from-shebang\necho hi\n",
        );

        #[cfg(windows)]
        let interpreter_path = dir.path().join("prek-test-interpreter.exe");
        #[cfg(not(windows))]
        let interpreter_path = dir.path().join("prek-test-interpreter");

        write_file(&interpreter_path, "");
        make_executable(&interpreter_path).expect("set executable bit");

        let paths = OsString::from(dir.path().as_os_str());
        let resolved = resolve_command(
            vec![OsString::from("./hook-script")],
            Some(paths.as_os_str()),
            dir.path(),
        );

        assert_eq!(
            resolved,
            vec![
                interpreter_path.into_os_string(),
                OsString::from("--from-shebang"),
                script_path.into_os_string(),
            ]
        );
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
            vec![script_path.as_os_str().to_owned()],
            Some(paths.as_os_str()),
            &CWD,
        );

        assert_eq!(resolved[0].as_os_str(), sh_path.as_os_str());
        assert_eq!(resolved[1].as_os_str(), script_path.as_os_str());
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
            vec![script_path.as_os_str().to_owned()],
            Some(paths.as_os_str()),
            &CWD,
        );

        let resolved_interp = Path::new(&resolved[0]);
        assert_eq!(resolved_interp, interp_path.as_path());
        assert_eq!(resolved[1].as_os_str(), script_path.as_os_str());
    }
}
