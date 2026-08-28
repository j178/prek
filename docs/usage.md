# Common Workflows

This page explains how to set up and use prek in a Git repository, including
creating or reusing a configuration, running hooks, and handling a hook that
prevents a commit.

## Set up the repository

First, [install prek](installation.md). The next step depends on whether the
repository already has a configuration.

### Use an existing configuration

If the repository already contains a supported configuration file, run this
command from the repository root:

```bash
prek install
```

This installs the Git shims selected by the repository's configuration so that
prek runs automatically during Git operations. By default, prek installs a
`pre-commit` shim.

If another tool already owns the hook, a normal install moves that hook to a
`.legacy` file and configures prek to run both implementations. This migration
mode lets you compare them before removing the old setup. When you are ready to
replace the legacy hook, run:

```bash
prek install -f
```

`prek uninstall` restores the legacy hook while migration mode is active. See
[Migrating from Other Hook Tools](migration.md#keep-the-existing-hook-during-rollout)
before using `--force` on a hook whose contents you have not reviewed.

Hook environments are normally prepared the first time they are needed. To
prepare them during setup instead, run:

```bash
prek install --prepare-hooks
```

### Create a configuration

If the repository does not have a configuration yet, run `prek init` from
anywhere in the Git worktree:

```bash
prek init
```

This creates a starter `prek.toml` at the Git worktree root and installs the
`pre-commit` Git shim in one step.

To place the configuration in an existing subdirectory, pass its path:

```bash
prek init packages/my-project
```

The directory must be inside the current Git worktree. Use `--format yaml` to
create `.pre-commit-config.yaml` instead of `prek.toml`. Add `--no-install` to
create only the configuration; run `prek install` later to install the Git hook
shims.

## What happens when you commit

Use Git as usual: stage the changes that belong in the commit, then commit them.

```console
$ git add settings.json
$ git commit -m "Update settings"
check json...............................................................Passed
mixed line ending........................................................Passed
[main 0123456] Update settings
 1 file changed, 1 insertion(+)
```

Before Git creates the commit, the `pre-commit` shim runs hooks configured for
that stage against the staged files. Unstaged changes are temporarily stashed
while the hooks run, so the hooks check the contents that will be committed. The
first run may take longer while prek downloads and prepares hook environments.

If every hook passes, Git creates the commit. If a hook fails or modifies files,
prek exits unsuccessfully and Git stops without creating the commit.

## When a hook reports a failure

A hook can reject a change and print the problem it found. For example:

```console
$ git commit -m "Update settings"
check json...............................................................Failed
- hook id: check-json
- exit code: 1

  settings.json: Failed to json decode (trailing comma at line 3 column 1)
```

Read the hook output, fix the reported problem, stage the corrected file, and
retry the commit:

```console
$ git add settings.json
$ git commit -m "Update settings"
check json...............................................................Passed
[main 0123456] Update settings
 1 file changed, 1 insertion(+)
```

The failed attempt did not create a partial commit. Other hooks may have reported
additional problems, so check the complete output before retrying.

## When a hook modifies files

Formatters and other fixing hooks can update files automatically. prek marks the
run as failed so that you can review and stage those changes before committing
them:

```console
$ git commit -m "Normalize line endings"
mixed line ending........................................................Failed
- hook id: mixed-line-ending
- exit code: 1
- files were modified by this hook

  Fixing mixed.txt
```

Inspect the changes, make any further edits you want, stage the final result, and
retry:

```console
$ git diff -- mixed.txt
$ git add mixed.txt
$ git commit -m "Normalize line endings"
mixed line ending........................................................Passed
[main 0123456] Normalize line endings
 1 file changed, 3 insertions(+), 3 deletions(-)
```

A hook can both modify files and report another error. In that case, keep the
automatic fixes you want and resolve the remaining error before staging and
retrying.

## Run hooks yourself

You do not need to create a commit to run the configured hooks.

Run hooks for the files currently staged in Git:

```bash
prek run
```

Run hooks against the whole repository, commonly before opening a pull request:

```bash
prek run --all-files
```

Run a single hook by ID:

```bash
prek run ruff
```

Inspect what would run without executing hooks or changing files:

```bash
prek run --dry-run
```

## Run a command in a hook environment

Use `prek exec` to run an explicit command with the toolchain, installed
dependencies, and environment variables prepared for one configured hook. The
hook environment is prepared first if necessary:

```bash
prek exec prettier -- prettier --stdin-filepath src/app.js < src/app.js
```

The hook selector must resolve to exactly one hook. In a workspace, use a
project-qualified selector when needed, for example:

```bash
prek exec frontend:prettier -- prettier --version
```

Everything after `--` is the command to execute. It replaces the hook's
configured `entry` and `args`; `prek exec` does not select files, schedule other
hooks, or stash changes. The child process keeps the current working directory
after applying `--cd`, inherits the terminal's standard input, output, and error
streams, and its exit status becomes the exit status of `prek exec`.

Backends whose hook entry defines a special execution mechanism, including
`docker`, `docker_image`, `fail`, `julia`, and `pygrep`, are not supported.
Builtin and meta hooks are also unsupported; `prek exec` reports an error for
these cases.

## Skip hooks for one commit

When one known hook is not applicable, skip only that hook by ID:

```bash
PREK_SKIP=ruff git commit -m "Update generated files"
```

`SKIP=ruff` is accepted for compatibility. In a workspace, the value can also be
a [project or project-qualified selector](workspace.md#project-and-hook-selection).

When the repository's policy permits it, Git can instead bypass the entire
`pre-commit` and `commit-msg` hook chain for one commit:

```bash
git commit --no-verify
```

This does not fix the reported problem, and the same checks may still fail in
continuous integration. Prefer fixing or explicitly resolving the hook failure
when possible.

## Inspect and debug

List the hooks and projects discovered in the current workspace:

```bash
prek list
```

Use verbose output when a hook fails without enough context:

```bash
prek run -vvv
```

prek also writes a log file to `~/.cache/prek/prek.log` by default. See
[Debugging](debugging.md) when reporting a prek problem.

## Maintain the repository's hook configuration

If you maintain the repository's prek setup, validate its configuration after
editing it:

```bash
prek validate-config prek.toml
```

Use `.pre-commit-config.yaml` instead if that is the repository's config file.

Inspect file type tags when `types`, `types_or`, or `exclude_types` filters do not
match as expected:

```bash
prek util identify path/to/file
```

Update pinned hook repository revisions or prepare hook environments without
touching Git shims:

```bash
prek update
prek prepare-hooks
```

Show or clean cached repositories, hook environments, and toolchains:

```bash
prek cache dir
prek cache gc
prek cache clean
```

## Where to go next

- [Configuration](configuration.md) covers config file formats, discovery, and
  validation.
- [Local Hooks](local-hooks.md) covers inline hook definitions, file passing,
  filtering, and working-directory behavior.
- [Continuous Integration](ci.md) covers full-repository and revision-range
  checks in CI.
- [Workspace Mode](workspace.md) covers monorepos and nested project configs.
- [CLI Reference](reference/cli.md) lists every command and option.
