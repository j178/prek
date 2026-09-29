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

## Recovering unstaged changes

When hooks run against staged files, prek saves unstaged changes in a patch and
restores them afterwards. If hook fixes conflict with that patch, prek rolls back
the fixes and restores your changes. Hooks should leave staging to you instead of
running `git add` themselves.

If restoration fails, prek stops the commit and prints the saved patch path and
the pre-hook index tree. The tree ID is also recorded at the top of the patch.
Keep the patch and export the tree before cleaning the prek cache or pruning Git
objects.

To recover without overwriting your current checkout, run this from the repository
root in Bash (or Git Bash on Windows). Replace `TREE` with the printed tree ID and
choose new paths outside any Git repository:

```bash
GIT_INDEX_FILE="/absolute/path/recovery.index" git read-tree TREE
GIT_INDEX_FILE="/absolute/path/recovery.index" git checkout-index --all --prefix="/absolute/path/recovered/"
git -C /absolute/path/recovered apply --check /absolute/path/saved.patch
git -C /absolute/path/recovered apply /absolute/path/saved.patch
```

These commands use a separate index and leave your checkout's index untouched.
Keep the trailing `/` on `--prefix`. The recovered files contain your original
staged content. Applying the patch adds your saved unstaged edits. Compare those
files with your checkout, copy back the changes you want, and review what to stage
before retrying the commit. Untracked files are not included in this recovery copy.

If `apply --check` still fails in the recovery directory, keep both the recovered
files and the patch. You can apply the matching parts with
`git -C /absolute/path/recovered apply --reject /absolute/path/saved.patch` and
resolve the remaining changes from the generated `.rej` files there.

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
