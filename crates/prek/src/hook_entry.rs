use std::ffi::{OsStr, OsString};
use std::ops::Deref;
use std::path::Path;

use itertools::intersperse;
use tempfile::TempDir;

use crate::config::Shell;
use crate::hook::Error;
use crate::languages::resolve_command;
use crate::store::Store;

const HOOK_REPO_PLACEHOLDER: &str = "{hook_repo}";

fn expand_placeholders(argv: &mut [OsString], repo_path: &Path) {
    for arg in argv {
        let Some(value) = arg.to_str() else {
            continue;
        };
        if !value.contains(HOOK_REPO_PLACEHOLDER) {
            continue;
        }

        *arg = intersperse(
            value.split(HOOK_REPO_PLACEHOLDER).map(OsStr::new),
            repo_path.as_os_str(),
        )
        .collect();
    }
}

#[derive(Debug)]
pub(crate) struct PreparedHookEntry {
    argv: Vec<OsString>,
    _temp_dir: Option<TempDir>,
}

impl PreparedHookEntry {
    pub(crate) fn argv(argv: Vec<OsString>) -> Self {
        Self {
            argv,
            _temp_dir: None,
        }
    }

    fn shell(argv: Vec<OsString>, temp_dir: TempDir) -> Self {
        Self {
            argv,
            _temp_dir: Some(temp_dir),
        }
    }

    pub(crate) fn argv_mut(&mut self) -> &mut Vec<OsString> {
        &mut self.argv
    }
}

impl Deref for PreparedHookEntry {
    type Target = [OsString];

    fn deref(&self) -> &Self::Target {
        &self.argv
    }
}

#[derive(Debug, Clone)]
pub(crate) enum HookEntry {
    Argv(ArgvHookEntry),
    Shell(ShellHookEntry),
}

impl HookEntry {
    pub(crate) fn new(hook: String, entry: String, shell: Option<Shell>) -> Self {
        match shell {
            Some(shell) => Self::Shell(ShellHookEntry { hook, entry, shell }),
            None => Self::Argv(ArgvHookEntry { hook, entry }),
        }
    }

    /// Split the entry and resolve the command by parsing its shebang.
    pub(crate) fn resolve(
        &self,
        repo_path: &Path,
        env_path: Option<&OsStr>,
        cwd: &Path,
        store: &Store,
    ) -> Result<PreparedHookEntry, Error> {
        match self {
            Self::Argv(entry) => entry.resolve(repo_path, env_path, cwd),
            Self::Shell(entry) => entry.resolve(env_path, cwd, store),
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
        cwd: &Path,
        store: &Store,
    ) -> Result<PreparedHookEntry, Error> {
        match self {
            Self::Argv(entry) => entry.resolve_script(repo_path, env_path, cwd),
            Self::Shell(entry) => entry.resolve(env_path, cwd, store),
        }
    }

    /// Return the argv-style entry, or `None` when `entry` is shell source.
    pub(crate) fn as_argv_entry(&self) -> Option<&ArgvHookEntry> {
        match self {
            Self::Argv(entry) => Some(entry),
            Self::Shell(_) => None,
        }
    }

    /// Return the argv-style entry after its execution path has rejected `shell`.
    ///
    /// # Panics
    ///
    /// Panics if this entry is shell source.
    pub(crate) fn expect_argv_entry(&self) -> &ArgvHookEntry {
        match self {
            Self::Argv(entry) => entry,
            Self::Shell(entry) => panic!(
                "hook `{}` uses `shell` in an argv-only execution path",
                entry.hook
            ),
        }
    }
}

/// An entry that can be interpreted as an argument vector without shell evaluation.
#[derive(Debug, Clone)]
pub(crate) struct ArgvHookEntry {
    hook: String,
    entry: String,
}

impl ArgvHookEntry {
    /// Split the entry and resolve the command by parsing its shebang.
    pub(crate) fn resolve(
        &self,
        repo_path: &Path,
        env_path: Option<&OsStr>,
        cwd: &Path,
    ) -> Result<PreparedHookEntry, Error> {
        let argv = self.split_expanded(repo_path)?;
        let argv = resolve_command(argv, env_path, cwd);

        Ok(PreparedHookEntry::argv(argv))
    }

    /// Resolve an argv-style `language: script` entry.
    fn resolve_script(
        &self,
        repo_path: &Path,
        env_path: Option<&OsStr>,
        cwd: &Path,
    ) -> Result<PreparedHookEntry, Error> {
        let mut argv = self.split_expanded(repo_path)?;
        argv[0] = repo_path.join(&argv[0]).into_os_string();
        let argv = resolve_command(argv, env_path, cwd);

        Ok(PreparedHookEntry::argv(argv))
    }

    /// Split the entry into an argument vector.
    pub(crate) fn split(&self) -> Result<Vec<OsString>, Error> {
        let splits = shlex::split(&self.entry).ok_or_else(|| Error::Hook {
            hook: self.hook.clone(),
            error: anyhow::anyhow!("Failed to parse entry `{}` as commands", self.entry),
        })?;
        if splits.is_empty() {
            return Err(Error::Hook {
                hook: self.hook.clone(),
                error: anyhow::anyhow!("Failed to parse entry: entry is empty"),
            });
        }
        Ok(splits.into_iter().map(OsString::from).collect())
    }

    /// Split the entry and expand `{hook_repo}` using `repo_path`.
    pub(crate) fn split_expanded(&self, repo_path: &Path) -> Result<Vec<OsString>, Error> {
        let mut argv = self.split()?;
        expand_placeholders(&mut argv, repo_path);
        Ok(argv)
    }

    /// Split the entry and append `args`.
    pub(crate) fn split_with_args(&self, args: &[String]) -> Result<Vec<OsString>, Error> {
        let mut split = self.split()?;
        split.extend(args.iter().map(OsString::from));
        Ok(split)
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
    fn resolve(
        &self,
        env_path: Option<&OsStr>,
        cwd: &Path,
        store: &Store,
    ) -> Result<PreparedHookEntry, Error> {
        let temp_dir = tempfile::tempdir_in(store.scratch_path())?;
        let script_path = temp_dir
            .path()
            .join("entry")
            .with_extension(self.shell.extension());
        fs_err::write(&script_path, &self.entry).map_err(|err| Error::Hook {
            hook: self.hook.clone(),
            error: anyhow::anyhow!(err).context("Failed to write shell entry script"),
        })?;

        let argv = resolve_command(self.shell.argv_for_script(&script_path), env_path, cwd);
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

    fn argv_for_script(self, script_path: &Path) -> Vec<OsString> {
        let script = script_path.as_os_str().to_owned();
        match self {
            Self::Sh => vec![OsString::from("sh"), OsString::from("-e"), script],
            Self::Bash => bash_argv(script),
            Self::Pwsh => powershell_argv("pwsh", script),
            Self::Powershell => powershell_argv("powershell", script),
            Self::Cmd => cmd_argv(script),
        }
    }
}

fn bash_argv(script: OsString) -> Vec<OsString> {
    // Avoid user startup files for deterministic hook behavior. `-e` fails on the first
    // failing command, and `-o pipefail` makes failing pipeline segments fail the script.
    const BASH_ARGV_PREFIX: &[&str] = &["bash", "--noprofile", "--norc", "-eo", "pipefail"];

    let mut argv = BASH_ARGV_PREFIX
        .iter()
        .map(OsString::from)
        .collect::<Vec<_>>();
    argv.push(script);
    argv
}

fn powershell_argv(command: &str, script: OsString) -> Vec<OsString> {
    let mut argv = vec![
        OsString::from(command),
        // Avoid user profile scripts and prompts in hook execution.
        OsString::from("-NoProfile"),
        OsString::from("-NonInteractive"),
    ];
    #[cfg(windows)]
    // Allow running prek's temporary script without changing the user's execution policy.
    argv.extend([OsString::from("-ExecutionPolicy"), OsString::from("Bypass")]);
    argv.extend([OsString::from("-File"), script]);
    argv
}

fn cmd_argv(script: OsString) -> Vec<OsString> {
    // `/D` disables AutoRun, `/E:ON` enables command extensions, `/V:OFF` disables
    // delayed expansion, `/S` normalizes quote handling, `/C` runs and exits, and
    // `CALL` executes the temporary script while preserving `%*` argument access.
    const CMD_ARGV_PREFIX: &[&str] = &["cmd", "/D", "/E:ON", "/V:OFF", "/S", "/C", "CALL"];

    let mut argv = CMD_ARGV_PREFIX
        .iter()
        .map(OsString::from)
        .collect::<Vec<_>>();
    argv.push(script);
    argv
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::path::Path;

    use super::expand_placeholders;

    #[test]
    fn expand_placeholders_replaces_hook_repo() {
        let repo_path = Path::new("hook repo");
        let mut argv = vec![
            OsString::from("tool"),
            OsString::from("--config={hook_repo}/path with spaces/config.toml"),
        ];

        let mut config_arg = OsString::from("--config=");
        config_arg.push(repo_path);
        config_arg.push("/path with spaces/config.toml");

        expand_placeholders(&mut argv, repo_path);

        assert_eq!(argv, vec![OsString::from("tool"), config_arg]);
    }
}
