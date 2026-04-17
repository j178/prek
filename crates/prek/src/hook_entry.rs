use std::ffi::OsStr;
use std::ops::Deref;
use std::path::Path;

use tempfile::TempDir;

use crate::config::Shell;
use crate::hook::Error;
use crate::languages::resolve_command;
use crate::store::Store;

#[derive(Debug)]
pub(crate) struct PreparedHookEntry {
    argv: Vec<String>,
    _temp_dir: Option<TempDir>,
}

impl PreparedHookEntry {
    fn direct(argv: Vec<String>) -> Self {
        Self {
            argv,
            _temp_dir: None,
        }
    }

    fn shell(argv: Vec<String>, temp_dir: TempDir) -> Self {
        Self {
            argv,
            _temp_dir: Some(temp_dir),
        }
    }

    pub(crate) fn argv(&self) -> &[String] {
        &self.argv
    }

    pub(crate) fn argv_mut(&mut self) -> &mut Vec<String> {
        &mut self.argv
    }
}

impl Deref for PreparedHookEntry {
    type Target = [String];

    fn deref(&self) -> &Self::Target {
        &self.argv
    }
}

#[derive(Debug, Clone)]
pub(crate) enum HookEntry {
    Direct(DirectHookEntry),
    Shell(ShellHookEntry),
}

impl HookEntry {
    pub(crate) fn new(hook: String, entry: String, shell: Option<Shell>) -> Self {
        match shell {
            Some(shell) => Self::Shell(ShellHookEntry { hook, entry, shell }),
            None => Self::Direct(DirectHookEntry { hook, entry }),
        }
    }

    /// Split the entry and resolve the command by parsing its shebang.
    pub(crate) fn resolve(
        &self,
        env_path: Option<&OsStr>,
        store: &Store,
    ) -> Result<PreparedHookEntry, Error> {
        match self {
            Self::Direct(entry) => entry.resolve(env_path),
            Self::Shell(entry) => entry.resolve(env_path, store),
        }
    }

    /// Resolve a `language: script` entry.
    ///
    /// Without `shell`, the first token is a repository-relative script path. With `shell`,
    /// the entry is shell source and is not rewritten as a script path.
    pub(crate) fn resolve_script(
        &self,
        repo_path: &Path,
        env_path: Option<&OsStr>,
        store: &Store,
    ) -> Result<PreparedHookEntry, Error> {
        match self {
            Self::Direct(entry) => entry.resolve_script(repo_path, env_path),
            Self::Shell(entry) => entry.resolve(env_path, store),
        }
    }

    /// Return the argv-style entry for execution paths that reject `shell` during validation.
    ///
    /// Panicking here means validation and execution support have diverged.
    pub(crate) fn expect_direct(&self) -> &DirectHookEntry {
        match self {
            Self::Direct(entry) => entry,
            Self::Shell(entry) => {
                panic!(
                    "Hook `{}` specified `shell`, but this execution path requires an argv-style entry",
                    entry.hook,
                );
            }
        }
    }

    pub(crate) fn shell(&self) -> Option<Shell> {
        match self {
            Self::Direct(_) => None,
            Self::Shell(entry) => Some(entry.shell),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct DirectHookEntry {
    hook: String,
    entry: String,
}

impl DirectHookEntry {
    /// Split the entry and resolve the command by parsing its shebang.
    fn resolve(&self, env_path: Option<&OsStr>) -> Result<PreparedHookEntry, Error> {
        let split = self.split()?;

        Ok(PreparedHookEntry::direct(resolve_command(split, env_path)))
    }

    /// Resolve a direct `language: script` entry.
    fn resolve_script(
        &self,
        repo_path: &Path,
        env_path: Option<&OsStr>,
    ) -> Result<PreparedHookEntry, Error> {
        let mut split = self.split()?;
        let cmd = repo_path.join(&split[0]);
        split[0] = cmd.to_string_lossy().to_string();

        Ok(PreparedHookEntry::direct(resolve_command(split, env_path)))
    }

    /// Split the entry into a list of commands.
    pub(crate) fn split(&self) -> Result<Vec<String>, Error> {
        let splits = shlex::split(&self.entry).ok_or_else(|| Error::Hook {
            hook: self.hook.clone(),
            error: anyhow::anyhow!("Failed to parse entry `{}` as commands", &self.entry),
        })?;
        if splits.is_empty() {
            return Err(Error::Hook {
                hook: self.hook.clone(),
                error: anyhow::anyhow!("Failed to parse entry: entry is empty"),
            });
        }
        Ok(splits)
    }

    /// Get the original entry string.
    pub(crate) fn raw(&self) -> &str {
        &self.entry
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ShellHookEntry {
    hook: String,
    entry: String,
    shell: Shell,
}

impl ShellHookEntry {
    fn resolve(&self, env_path: Option<&OsStr>, store: &Store) -> Result<PreparedHookEntry, Error> {
        let temp_dir = tempfile::tempdir_in(store.scratch_path())?;
        let script_path = temp_dir
            .path()
            .join("entry")
            .with_extension(self.shell.extension());
        fs_err::write(&script_path, &self.entry).map_err(|err| Error::Hook {
            hook: self.hook.clone(),
            error: anyhow::anyhow!(err).context("Failed to write shell entry script"),
        })?;

        let argv = resolve_command(self.shell.argv_for_script(&script_path), env_path);
        Ok(PreparedHookEntry::shell(argv, temp_dir))
    }
}

impl Shell {
    fn extension(self) -> &'static str {
        match self {
            Self::Sh | Self::Bash => "sh",
            Self::Pwsh | Self::Powershell => "ps1",
            Self::Cmd => "cmd",
        }
    }

    fn argv_for_script(self, script_path: &Path) -> Vec<String> {
        let script = script_path.to_string_lossy().to_string();
        match self {
            Self::Sh => vec!["sh".to_string(), "-e".to_string(), script],
            Self::Bash => bash_argv(script),
            Self::Pwsh => powershell_argv("pwsh", script),
            Self::Powershell => powershell_argv("powershell", script),
            Self::Cmd => cmd_argv(script),
        }
    }
}

fn bash_argv(script: String) -> Vec<String> {
    // Avoid user startup files for deterministic hook behavior. `-e` fails on the first
    // failing command, and `-o pipefail` makes failing pipeline segments fail the script.
    const BASH_ARGV_PREFIX: &[&str] = &["bash", "--noprofile", "--norc", "-eo", "pipefail"];

    let mut argv = BASH_ARGV_PREFIX
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    argv.push(script);
    argv
}

fn powershell_argv(command: &str, script: String) -> Vec<String> {
    let mut argv = vec![
        command.to_string(),
        // Avoid user profile scripts and prompts in hook execution.
        "-NoProfile".to_string(),
        "-NonInteractive".to_string(),
    ];
    #[cfg(windows)]
    // Allow running prek's temporary script without changing the user's execution policy.
    argv.extend(["-ExecutionPolicy".to_string(), "Bypass".to_string()]);
    argv.extend(["-File".to_string(), script]);
    argv
}

fn cmd_argv(script: String) -> Vec<String> {
    // `/D` disables AutoRun, `/E:ON` enables command extensions, `/V:OFF` disables
    // delayed expansion, `/S` normalizes quote handling, `/C` runs and exits, and
    // `CALL` executes the temporary script while preserving `%*` argument access.
    const CMD_ARGV_PREFIX: &[&str] = &["cmd", "/D", "/E:ON", "/V:OFF", "/S", "/C", "CALL"];

    let mut argv = CMD_ARGV_PREFIX
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    argv.push(script);
    argv
}
