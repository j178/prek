# Workspace Reference

A workspace consists of a root project and any discovered projects below it.
Each project has one configuration file. For setup examples and everyday
commands, see [Monorepos](../monorepos.md).

## Discovery

Without `--config`, prek searches upward from the current working directory for
`prek.toml`, `.pre-commit-config.yaml`, or `.pre-commit-config.yml`. It stops at
the first directory with a config, which becomes the workspace root. The search
never goes above the Git repository root.

From the workspace root, prek recursively discovers projects in subdirectories.
The workspace root can be inside the Git repository; it need not be the Git
root. `-C` / `--cd` changes the working directory before discovery, so it can
change which projects are found.

When several supported config files exist in one directory, prek uses the
[configuration filename precedence](../configuration.md#choose-a-config-file) to choose one.

### Exclusions

Project discovery respects:

- `.gitignore`, `.git/info/exclude`, and global Git ignore rules.
- `.prekignore` files, which use Git ignore syntax and apply to their directory
  and its descendants.

Discovery skips hidden directories, Cookiecutter template directories such as
`{{cookiecutter.project_slug}}`, and Git submodules. It does not follow directory
symlinks.

Discovery exclusions control which configs become projects. They do not remove
files from an ancestor project's candidate files. Use `files`, `exclude`, and
type filters to control the files passed to that project's hooks.

### Refreshing discovery

prek caches discovered projects. Changes to or removal of known config files
invalidate the cache. New configs and changes to `.prekignore` may need an
explicit refresh:

```bash
prek list --refresh
prek run --all-files --refresh
```

For staged-file runs, config changes must also be staged. See
[Debugging](../debugging.md#a-config-or-workspace-change-is-not-detected).

## Files and working directories

The run mode selects candidate files: staged files by default, all tracked
files with `--all-files`, or an explicit list with `--files`.

Each project receives candidate files below its directory and applies its own
top-level and hook-level filters. Filenames and path filters are relative to
that project's directory, which is also the working directory for its hooks.
A frontend hook cannot select sibling backend files through its file filters.
This file selection is not a filesystem sandbox for the hook process.

Parent and child projects apply their filters independently. A parent's
`exclude` does not disable a child project. By default, a file is eligible for
hooks in every discovered ancestor project, subject to each project's filters.
A child config does not override or inherit its parent's configuration.

## Execution order

Projects run from deepest to shallowest. For example, `src/backend/` runs before
`src/`, which runs before the workspace root.

Projects at the same depth may run concurrently. Their selected file sets do
not overlap, but hooks that access shared resources outside their project can
still contend. Hook concurrency is bounded by
[`PREK_CONCURRENT_HOOKS`](environment-variables.md#prek_concurrent_hooks).

## Orphan projects

Setting [`orphan`](configuration.md#prek-only-orphan) to `true` excludes files
under that project from all ancestor projects. Descendant projects still apply
their own configurations.

For example, with configs at the root, `src/`, and `src/backend/`:

| File | Default eligible projects | With `orphan = true` in `src/backend/` |
| -- | -- | -- |
| `src/backend/app.py` | `src/backend/`, `src/`, root | `src/backend/` |
| `src/app.py` | `src/`, root | `src/`, root |
| `README.md` | Root | Root |

The exclusion applies even when the orphan project's hooks are skipped,
unselected, or match no files. Skipping an orphan does not return its files to
ancestor projects.

## Selectors

Positional selectors choose which hooks to run; `--skip` excludes matches.
Both accept the same forms:

| Form | Matches |
| -- | -- |
| `<hook-id>` | That hook ID or alias across projects |
| `<project-path>/` | All hooks in that project and its descendants |
| `<project-path>:<hook-id>` | That hook ID or alias in exactly that project |
| `:<hook-id>` | That hook ID or alias across projects, including IDs containing `:` |

Relative project paths resolve from the current working directory after `-C`
/ `--cd`. Paths must be inside the discovered workspace. Use a trailing slash
for project selectors: a bare name such as `frontend` is interpreted as a hook
ID. `.` and paths containing `/` are also interpreted as project selectors.

A colon separates the project path from the hook ID. For a hook named
`lint:ruff`, use `:lint:ruff` to match it across projects, or
`frontend:lint:ruff` to select it in `frontend`. From the workspace root,
`.:check-json` selects only the root project's `check-json` hook.

Multiple positional selectors include the union of their matches. Skips take
precedence over those selections. Selecting or skipping a project includes
its descendants:

```bash
# Select two projects, then exclude a nested project
prek run frontend/ backend/ --skip frontend/legacy/

# Select different hooks from two projects
prek run frontend:lint backend:format

# Select a hook across projects, except in tests
prek run lint --skip tests/
```

`PREK_SKIP` and `SKIP` accept comma-separated selectors:

```bash
PREK_SKIP=frontend/,backend:format prek run
```

The precedence is `--skip` > `PREK_SKIP` > `SKIP`. The highest-precedence
supplied source determines the skip list; these sources are not combined.

## Single config mode

Passing `-c` / `--config` disables workspace discovery. Only that config is
loaded, even if it resides in a subdirectory:

```bash
prek run --config frontend/prek.toml
```

Hooks run from the Git repository root. Candidate files come from the Git
repository according to the run mode, and the selected config's filters apply
to repository-relative paths. Project selectors do not change these working
directory and file-scope rules.

## Git hook installation

Run `prek install` from the repository root to install Git shims for the whole
repository. Installation uses the root config's `default_install_hook_types`;
it does not combine values from nested configs. Explicit `--hook-type` options
take precedence. See [`prek install`](cli.md#prek-install).
