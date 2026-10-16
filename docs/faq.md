# FAQ

## Why did no hook run?

`prek run` checks staged files by default. If nothing relevant is staged, or all
staged files are excluded by the hook's filters, there is nothing to run. Try:

```bash
prek run --all-files --dry-run
```

Also check the hook's `stages`, file filters, and any `PREK_SKIP` or `SKIP`
value. See [Debugging](debugging.md#a-hook-is-skipped-or-receives-no-files) for
the full checklist.

## Why is the first run slower?

The first run may clone hook repositories, select toolchains, and prepare
isolated environments. Later runs reuse those results from `PREK_HOME`. CI jobs
with an empty filesystem pay this setup cost again unless they restore a
compatible cache.

## Why did a formatter change files but fail the commit?

This prevents Git from committing changes you have not reviewed. Inspect the
formatter's changes, stage the intended result, then commit again:

```bash
git diff
git add path/to/file
git commit
```

The next run checks the newly staged content. See
[When a hook modifies files](running-hooks.md#when-a-hook-modifies-files).

## Why must a changed config be staged?

For a normal staged-file run, prek temporarily hides unstaged changes so hooks
see the exact content that Git would commit. A staged source file paired with an
unstaged config would mix two snapshots, so prek asks you to stage new or
changed config files first.

## How do I skip one hook temporarily?

Set a comma-separated selector list for that command:

```bash
PREK_SKIP=ruff git commit -m "Update generated files"
```

Use this only when the repository's policy allows it. Prefer a selective skip
over `git commit --no-verify`, which bypasses the entire Git hook chain.

## I updated `.prekignore`, why didn't discovery change?

Workspace discovery is cached. If you edited `.prekignore`, run the command with `--refresh` to force a fresh project discovery so the changes are picked up. For example:

```bash
prek run --refresh
```

## What does `prek install --prepare-hooks` do?

In short, it installs the Git shims **and** prepares the environments for the hooks managed by prek. It is inherited from the original Python-based `pre-commit` tool to maintain compatibility with existing workflows.

It's a little confusing because it refers to two different kinds of hooks:

1. **Git shims** – Scripts placed in Git's effective hooks directory, usually `.git/hooks/` unless `core.hooksPath` points elsewhere. Both prek and upstream `pre-commit` drop a small shim here so Git automatically runs them on `git commit`.
2. **prek-managed hooks** – The tools listed in `.pre-commit-config.yaml`. When prek runs, it executes these hooks and prepares whatever runtime they need (for example, creating a Python virtual environment and installing the hook's dependencies before execution).

Running `prek install` installs the first type: it writes the Git shim so that Git knows to call prek. Which Git shims get installed is determined by `--hook-type` or `default_install_hook_types` in the config file, and defaults to `pre-commit` if neither is set. This is not affected by a hook's `stages` field in the config: `stages` controls when a configured hook may run, not which Git shims `prek install` writes.

Adding `--prepare-hooks` tells prek to do that **and** proactively create the environments and caches required by the hooks that prek manages. That way, the next time Git invokes prek through the shim, the managed hooks are ready to run without additional setup. The older `--install-hooks` spelling remains as an alias.

## Does prek work with Jujutsu (jj)?

prek works with Git-backed [Jujutsu](https://jj-vcs.github.io/jj/) repositories and
detects them automatically, including secondary workspaces created with
`jj workspace add`. No extra configuration is needed. A repository on jj's native
backend has no Git store for prek to drive, so prek reports that instead of
guessing at a repository.

Inside a jj workspace, prek:

- Resolves the backing Git directory from `.jj/repo/store/git_target`, so its Git
  commands work even when the workspace has no `.git` of its own.
- Runs the default `prek run` on the files changed in the current working-copy
  changeset, because jj has no staging area separate from the changeset.
- Skips the Git index stash and the staged-config check, which do not apply to jj.
- Points a hook's own Git commands at that backing store, so they run against your
  workspace instead of failing to find a repository.

`--all-files`, `--files`, and `--from-ref`/`--to-ref` behave as they do in a Git
repository. In a colocated workspace, prek still detects jj, so even an installed
Git hook checks the jj working-copy changeset rather than Git's staged files.

!!! note

    A few checks read Git's index or merge state directly rather than a file list,
    so they have limited support in jj workspaces:

    - `no-commit-to-branch` is skipped, since jj has no current branch that maps
      to Git's `HEAD`.

    - `forbid-new-submodules` does not detect newly added submodules, because it
      reads `git diff --cached`.

    - `PRE_COMMIT_FROM_REF`/`PRE_COMMIT_TO_REF` name the selected commits by their
      backing-store commit IDs, since Git cannot resolve `HEAD`/`HEAD~1` or a revset
      here. prek exports no pair when a selection names jj's root commit, whose
      all-zero ID Git rejects in a diff range, such as `--last-commit` on a first
      commit.

    - `check-merge-conflict` does not fire on a jj conflict unless it runs with
      `--assume-in-merge`, because it looks for Git merge-state files.

    - `destroyed-symlinks` compares the index with the working copy through
      `git status`, and `check-executables-have-shebangs` reads the index when
      `core.fileMode` is off, so both can miss working-copy files.

    - a working-copy path that is not valid UTF-8 does not reach file selection, so hooks
      never receive it; a path holding a newline does.

    - prek resolves its backend once, from the directory it runs in. A Git repository
      nested inside a jj workspace is not resolved on its own, so file queries that
      reach into one (for example `--files nested/big.txt`) use the outer backend. Run
      prek from inside the nested repository to use its own.

## How does `prek install` interact with `core.hooksPath` and worktrees?

If `core.hooksPath` is set in repo-local (`git config --local`) or worktree-local (`git config --worktree`) config, `prek install` and `prek uninstall` will honor it and operate on Git's effective hooks directory.

If `core.hooksPath` is only configured globally or system-wide, prek refuses to install or uninstall by default. That setting may be shared across repositories, so prek avoids mutating a hook location it does not own.

Use `prek install --force` to install into the repository's default hooks directory anyway. Use `--git-dir <GIT_DIR>` instead when you need to choose an explicit installation target.

## How do I use hooks from private repositories?

prek supports cloning hooks from private repositories that require authentication.
prek first clones with interactive terminal prompts disabled so non-interactive runs do
not hang. If a clone fails with an authentication error and prek is not running in CI,
it retries with terminal prompts enabled so Git can ask for credentials. In CI,
interactive prompts remain disabled, so you still need to configure credentials via
credential helpers, environment variables, or SSH.

### Option 1: Credential helpers (recommended)

If you use GitHub CLI, Git Credential Manager, macOS Keychain, or similar tools,
authentication often works automatically with no extra configuration:

```shell
# GitHub CLI users: configure git to use gh for credentials
gh auth setup-git

# Download configured hook repositories and prepare their environments
prek prepare-hooks
```

Other credential helpers that work out of the box:

- **macOS**: Keychain (`credential.helper=osxkeychain`)
- **Windows**: Git Credential Manager (`credential.helper=manager`)
- **Linux**: GNOME Keyring, KWallet, or `credential.helper=store` when storing credentials in plaintext is acceptable

You can also use `GIT_ASKPASS` to point to a custom credential program:

```shell
export GIT_ASKPASS=/path/to/credential-script
```

### Option 2: SSH URLs

Use SSH URLs in your `.pre-commit-config.yaml` instead of HTTPS:

```yaml
repos:
  - repo: git@github.com:myorg/private-hooks.git
    rev: v1.0.0
    hooks:
      - id: my-hook
```

This works automatically if you have SSH keys configured with an agent.

### Option 3: URL rewriting with tokens (for CI)

In CI environments without credential helpers, use environment variables to
rewrite HTTPS URLs to include credentials:

```shell
# GitHub Actions example
export GIT_CONFIG_COUNT=1
export GIT_CONFIG_KEY_0="url.https://oauth2:${GITHUB_TOKEN}@github.com/.insteadOf"
export GIT_CONFIG_VALUE_0="https://github.com/"

# Or using GIT_CONFIG_PARAMETERS (more compact)
export GIT_CONFIG_PARAMETERS="'url.https://oauth2:${GITHUB_TOKEN}@github.com/.insteadOf=https://github.com/'"
```

> **Security note:** Be careful with tokens in environment variables. Ensure your
> CI system masks secrets in logs.

## How is `prek` pronounced?

Like "wreck", but with a "p" sound instead of the "w" at the beginning.
The name comes from saying "pre-commit" and stopping right after the hard "k"
sound; it can also be read as short for "pre-check".
