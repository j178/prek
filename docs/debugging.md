# Debugging

Start by recording the versions and checking what prek discovered:

```bash
prek --version
git --version
prek list
prek run --dry-run
```

Then rerun the smallest failing command with verbose tracing:

```bash
prek run check-yaml -vvv
```

Replace `check-yaml` with the ID of the failing hook.

## A hook does not run during Git operations

Confirm that the expected Git shim is installed:

```bash
prek install
git config --show-origin --get core.hooksPath
```

`prek install` defaults to the `pre-commit` shim. Other stages require either
`default_install_hook_types` in the config or an explicit
`prek install --hook-type <stage>`. A hook's `stages` setting controls whether it
is eligible to run, but does not install the corresponding Git shim.

If another tool already owned the hook, check the install output for migration
mode. prek may be running both its own hook and a preserved `.legacy` hook.

## A hook is skipped or receives no files

`prek run` without a file-selection option checks the files staged in Git. Try
the whole repository and inspect the selection without executing hooks:

```bash
prek run --all-files --dry-run
```

Check `files`, `exclude`, `types`, `types_or`, `exclude_types`, `stages`, and any
`PREK_SKIP` or `SKIP` value. To see the type tags prek assigns to a path, run:

```bash
prek util identify path/to/file
```

In a workspace, use `prek list` to confirm the project and use a
[project-qualified selector](reference/workspace.md#selectors) when hook
IDs are repeated.

## A config or workspace change is not detected

New and changed config files must be staged for the default staged-file run.
This keeps config discovery and hook execution on the same snapshot:

```bash
git add prek.toml
prek run
```

Use the repository's YAML config filename instead when applicable.

If you added a nested config or changed `.prekignore`, rebuild workspace
discovery with:

```bash
prek run --refresh
```

## Hook installation or downloads fail

Use `-vvv` to identify whether the failing step is Git authentication, TLS,
toolchain download, or the language package manager. Then check:

- [Private repository authentication](faq.md#how-do-i-use-hooks-from-private-repositories)
- Proxy and certificate variables in the
  [Environment Variable Reference](reference/environment-variables.md#related-external-variables)
- Language-specific prerequisites in [Language Support](reference/language-support.md)
- The checksum and trust boundary in the [Security Guide](security.md)

If a Rust-native fast path behaves differently from the pinned hook, compare it
with:

```bash
PREK_NO_FAST_PATH=1 prek run check-yaml --all-files
```

## A hook command works in a terminal but fails in prek

prek runs hook commands directly, without an interactive shell. Shell aliases
and functions are not available, and prek does not load shell startup files such
as `.bashrc` or `.zshrc`. A tool initialized by those files may work in your
terminal but be unavailable to Git hooks, GUI Git clients, or CI.

Check the command and working directory in the error, together with the original
OS error. "No such file or directory" can mean a missing executable, working
directory, or script interpreter; it does not necessarily mean the command is
absent from `PATH`.

Make sure the executable is installed and available on `PATH` in the environment
that launches prek. For `language: system` hooks, prek does not install the tool
for you. Use an absolute executable path or a wrapper script that explicitly
sets up the required environment instead of relying on an alias or function.
Relative paths are resolved from the hook's working directory.

## Cache problems

Inspect the cache before removing anything:

```bash
prek cache dir
prek cache size
```

To remove unused cached repositories and environments, run:

```bash
prek cache gc
```

`prek cache clean` removes cached hook repositories, environments, and managed
tools, so the next run must download and prepare them again. Use it only after a
normal retry and garbage collection do not resolve a corrupted environment.

## Logs and bug reports

prek writes trace logs to `$PREK_HOME/prek.log`. By default this is
`~/.cache/prek/prek.log` on macOS and Linux, and the prek directory under
`%LOCALAPPDATA%` on Windows. Choose a separate file for one reproduction with:

```bash
prek --log-file prek-debug.log run check-yaml -vvv
```

When reporting a bug, include the smallest reproducer, command, complete error,
prek version, operating system, and relevant log section. Remove credentials,
private repository URLs, user paths, and sensitive hook output before sharing a
log publicly.
