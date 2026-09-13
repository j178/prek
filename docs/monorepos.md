# Monorepos

Keep shared checks in the repository root and give individual projects their own
hook configurations. When you run prek from the root, it discovers the nested
configs and runs their hooks too. This is called **workspace mode**; no workspace
manifest or opt-in setting is needed.

## Add a project configuration

Start with a root config for checks that apply across the repository. Add a
config in each directory that needs its own checks:

```text
my-repo/
├── .git/
├── prek.toml
└── frontend/
    ├── prek.toml
    └── package.json
```

For example, trim trailing whitespace throughout the repository:

```toml title="prek.toml"
[[repos]]
repo = "builtin"
hooks = [{ id = "trailing-whitespace" }]
```

Then check JSON syntax in `frontend/`:

```toml title="frontend/prek.toml"
[[repos]]
repo = "builtin"
hooks = [{ id = "check-json" }]
```

Each directory containing a config is a **project**. Projects can also use
`.pre-commit-config.yaml` or `.pre-commit-config.yml`, and can contain further
nested projects. See [Local Hooks](local-hooks.md) to run a project's existing
linter or formatter.

If you prefer a starter config, `prek init frontend --no-install` creates one in
an existing project directory. Edit it to choose that project's hooks.

## Run checks across the repository

Run these commands from the repository root:

```bash
prek list
prek run --all-files
```

`prek list` shows the discovered projects and hooks. In this example,
`frontend/package.json` is checked by both `frontend`'s JSON hook and the root's
whitespace hook. A child config adds checks; it does not override its parent.

To run hooks automatically when committing, install the Git shim once from the
repository root:

```bash
prek install
```

Stage the config files along with your changes before committing or running
`prek run` without `--all-files`. That run checks staged files. Configure
`default_install_hook_types` in the root config if you need Git stages other
than `pre-commit`; installation does not combine this setting from subprojects.

## Run or skip a project

From the repository root, select a project with a trailing slash, or qualify a
hook ID with its project path:

```bash
# Run hooks from frontend and any projects nested inside it
prek run frontend/ --all-files

# Run only frontend's JSON check
prek run frontend:check-json --all-files

# Run every hook named check-json in the workspace
prek run check-json --all-files

# Run the other projects' hooks
prek run --skip frontend/ --all-files
```

Selecting `frontend/` leaves out the root project's hooks. Skipping `frontend/`
leaves the root's hooks eligible to check files inside `frontend/`.

To skip a project for one commit, use `PREK_SKIP=frontend/ git commit`.
See the [workspace reference](reference/workspace.md#selectors) for combining
selectors, hook aliases, and skip precedence.

## Choose which checks apply to a project's files

Hooks run in their project's directory. The filenames they receive and the
`files` / `exclude` patterns are relative to that directory. For example, a
frontend hook sees `package.json`, while a root hook sees
`frontend/package.json`.

Each project applies its filters independently. A root-level `exclude` can
prevent a root hook from checking `frontend/`, but does not disable the
frontend project's hooks. Put checks that need files from several projects in
a config at their common ancestor.

If a project should handle its files without any parent checks, add `orphan` at
the top level of its config, before `[[repos]]`:

=== "prek.toml"

    ```toml
    orphan = true
    ```

=== ".pre-commit-config.yaml"

    ```yaml
    orphan: true
    ```

For `frontend/`, this excludes its files from the root's hooks. Keep the shared
checks you still want in the frontend config. Even if you skip an orphan
project, its files do not fall back to parent hooks. See
[orphan projects](reference/workspace.md#orphan-projects) for the full rules.

## Exclude a directory from project discovery

To stop discovering configs under a directory, add it to `.prekignore` at the
repository root:

```gitignore title=".prekignore"
legacy/
```

Then run `prek run --all-files --refresh`. This excludes projects under
`legacy/`; root hooks can still check files there. Use the root config's
`exclude` if those files should also be excluded from its checks.

Discovery already respects Git ignore rules. `.prekignore` adds exclusions
without ignoring the files in Git. See the
[discovery rules](reference/workspace.md#discovery) for other exclusions.

## Work from a project directory

Running inside `frontend/` uses its config as the workspace root and discovers
projects below it. You can do the same from the repository root with:

```bash
prek -C frontend run --all-files
```

Use `-C` to change the working directory before discovery. Use `--config` when
you want to run exactly one config with paths relative to the Git repository
root:

| Command from the repository root | Configs used | Hook working directory |
| -- | -- | -- |
| `prek run frontend/` | Frontend and its nested projects | Each project's directory |
| `prek -C frontend run` | Frontend and its nested projects | Each project's directory |
| `prek run --config frontend/prek.toml` | Only `frontend/prek.toml` | Git repository root |

Passing `--config` disables workspace discovery, even when the config is in a
subdirectory. See [single config mode](reference/workspace.md#single-config-mode)
for file scope and filtering.

## A new project is not being discovered

After adding a config or changing `.prekignore`, refresh discovery:

```bash
prek list --refresh
prek run --all-files --refresh
```

If the project is still missing, check that it is below the workspace root and
not excluded by the [discovery rules](reference/workspace.md#discovery). For
staged-file runs, also stage the new config. See
[Debugging](debugging.md#a-config-or-workspace-change-is-not-detected) for more
help.
