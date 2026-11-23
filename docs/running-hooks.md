# Running Hooks

Run hooks on demand or let Git run them when you commit. If you are setting up
prek for the first time, start with the [Quickstart](quickstart.md).

For a repository that already has a config, enable its Git hooks in your checkout
by running `prek install` from the repository root. If another tool already owns
the hook, see [migration mode](migration.md#keep-the-existing-hook-during-rollout).

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

## Prepare hook environments

prek normally prepares a hook's environment the first time it is needed. To
prepare environments in advance while setting up a checkout, run:

```bash
prek prepare-hooks
```

When setting up a checkout, `prek install --prepare-hooks` installs the Git
shims and prepares environments together. See [Debugging](debugging.md#cache-problems)
for inspecting and cleaning cached environments.

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
while the hooks run, so the hooks check the contents that will be committed.

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

## Skip hooks for one commit

When one known hook is not applicable, skip only that hook by ID:

```bash
PREK_SKIP=ruff git commit -m "Update generated files"
```

`SKIP=ruff` is accepted for compatibility. In a workspace, the value can also be
a [project or project-qualified selector](reference/workspace.md#selectors).

When the repository's policy permits it, Git can instead bypass the entire
`pre-commit` and `commit-msg` hook chain for one commit:

```bash
git commit --no-verify
```

This does not fix the reported problem, and the same checks may still fail in
continuous integration. Prefer fixing or explicitly resolving the hook failure
when possible.

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

The command runs in your current directory with the selected hook's environment.
See [`prek exec`](reference/cli.md#prek-exec) for supported hooks and complete
execution behavior.

## Inspect and debug

List the hooks and projects discovered in the current workspace:

```bash
prek list
```

Use verbose output when a hook fails without enough context:

```bash
prek run -vvv
```

See [Debugging](debugging.md) for logs, cache problems, and hooks that do not run
as expected.

## Where to go next

- [Configuration](configuration.md) covers config file formats, discovery,
  validation, and updating hooks.
- [Local Hooks](local-hooks.md) covers inline hook definitions, file passing,
  filtering, and working-directory behavior.
- [Continuous Integration](ci.md) covers full-repository and revision-range
  checks in CI.
- [Monorepos](monorepos.md) covers nested project configs and project selection.
- [CLI Reference](reference/cli.md) lists every command and option.
