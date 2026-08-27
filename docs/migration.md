# Migrating from Other Hook Tools

Move one hook at a time when replacing an existing setup. Keep the old hook
available until the equivalent `prek` hook has run successfully on the whole
repository and in CI.

## From pre-commit

Start with the two-step path in the [Quickstart](quickstart.md#already-using-pre-commit).
The [Compatibility](compatibility.md) and [Differences](diff.md) pages cover the
cases worth checking when a repository depends on less common behavior.

## From lint-staged or Husky

A lint-staged command usually becomes a [`repo = "local"`](local-hooks.md) hook.
For example, this hook runs the project's ESLint installation and lets `prek`
append matching filenames:

=== "prek.toml"

    ```toml
    [[repos]]
    repo = "local"

    [[repos.hooks]]
    id = "eslint"
    name = "eslint"
    language = "system"
    entry = "npm exec -- eslint"
    files = "\\.[cm]?[jt]sx?$"
    pass_filenames = true
    ```

=== ".pre-commit-config.yaml"

    ```yaml
    repos:
      - repo: local
        hooks:
          - id: eslint
            name: eslint
            language: system
            entry: npm exec -- eslint
            files: '\.[cm]?[jt]sx?$'
            pass_filenames: true
    ```

The project must install its Node dependencies before this hook runs. Map the
rest of the setup as follows:

| Existing concept | prek equivalent |
| -- | -- |
| lint-staged file glob | `files`, `types`, `types_or`, and `exclude` |
| Filenames appended to a command | `pass_filenames = true`, which is the default |
| Command discovers its own files | `pass_filenames = false` |
| Shell pipeline or expansion | Prefer a direct command; otherwise set `shell` explicitly |
| Husky hook script | A Git stage plus `prek install` |

If a Husky script also performs unrelated work, keep that work in a project
script and call the script from a local hook. This keeps the Git shim small and
makes the command easy to run outside Git.

## From Lefthook

Translate each Lefthook command into a local hook:

| Lefthook concept | prek equivalent |
| -- | -- |
| Hook name such as `pre-commit` or `pre-push` | Hook `stages` and an installed Git shim |
| `commands.<name>.run` | Local hook `entry` and `args` |
| `{staged_files}` | The default `pass_filenames = true` behavior |
| `glob` and `exclude` | `files`, `types`, and `exclude` |
| Parallel command groups | Hooks with the same `priority` |

Installing a shim and making a hook eligible for that stage are separate
choices. For example:

```toml
default_install_hook_types = ["pre-commit", "pre-push"]

[[repos]]
repo = "local"

[[repos.hooks]]
id = "tests"
name = "tests"
language = "system"
entry = "cargo test"
pass_filenames = false
stages = ["pre-push"]
```

Hooks remain sequential when `priority` is omitted. Give independent hooks the
same explicit [`priority`](reference/configuration.md#priority) only after
checking that they do not modify the same files or contend for shared state.

## Keep the existing hook during rollout

If `.git/hooks/<hook-name>` already belongs to another tool, a normal
`prek install` moves it to `<hook-name>.legacy` and installs prek in migration
mode. The prek shim runs both hook implementations.

```bash
prek install
```

When the migration is complete, replace the legacy hook:

```bash
prek install --force
```

Before using `--force`, make sure the old hook contains no checks that are still
needed. If you uninstall while migration mode is active, `prek uninstall`
restores the legacy hook to its original path.

## Migration checklist

1. Put each existing check in a local or remote hook.
2. Confirm file filtering and whether the command accepts filenames.
3. Configure every Git stage that the old tool handled.
4. Stage the config and run `prek run --all-files`.
5. Add the same command to [CI](ci.md).
6. Install the Git shims, initially preserving the old hook if useful.
7. Remove the old tool and dependencies only after local and CI runs agree.
