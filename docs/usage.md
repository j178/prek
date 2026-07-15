# Common Workflows

This page explains how to use prek in a repository that already contains a
`prek.toml` or `.pre-commit-config.yaml`, including setup, running hooks, and
handling a hook that prevents a commit.

## Set up the repository

First, [install prek](installation.md), then run this command from the repository
root:

```bash
prek install
```

This installs the Git shims selected by the repository's configuration so that
prek runs automatically during Git operations. If the repository does not
select any hook types, prek installs a `pre-commit` shim by default. If the
repository previously used `pre-commit` and already has its shims installed,
replace them once:

```bash
prek install -f
```

Hook environments are normally prepared the first time they are needed. To
prepare them during setup instead, run:

```bash
prek install --prepare-hooks
```

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

## Skip hooks for one commit

When the repository's policy permits it, Git can bypass the `pre-commit` and
`commit-msg` hooks for one commit:

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
- [Workspace Mode](workspace.md) covers monorepos and nested project configs.
- [CLI Reference](reference/cli.md) lists every command and option.
