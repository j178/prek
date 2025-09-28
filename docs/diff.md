# Difference from pre-commit

## General differences

- `prek` supports both `.pre-commit-config.yaml` and `.pre-commit-config.yml` configuration files.
- `prek` implements some common hooks from `pre-commit-hooks` in Rust for better performance.
- `prek` uses `~/.cache/prek` as the default cache directory for repos, environments and toolchains.
- `prek` decoupled hook environment from their repositories, allowing shared toolchains and environments across hooks.
- `prek` supports `language_version` as a semver specifier and automatically installs the required toolchains.

## Workspace mode

`prek` supports workspace mode, allowing you to run hooks for multiple projects in a single command. Each subproject can have its own `.pre-commit-config.yaml` file.

See [Workspace Mode](./workspace.md) for more information.

## Language support

### Python

#### Dependency Management with `uv`

`prek` uses `uv` for creating virtual environments and installing dependencies:

- First tries to find `uv` in the system PATH
- If not found, automatically installs `uv` from the best available source (GitHub releases, PyPI, or mirrors)
- Automatically installs the required Python version if it's not already available

#### PEP 723 Inline Script Metadata Support

For Python hooks **without** `additional_dependencies`, `prek` can read PEP 723 inline metadata from the script specified in the `entry` field.

**Example:**

`.pre-commit-config.yaml`:

```yaml
repos:
  - repo: local
    hooks:
      - id: echo
        name: echo
        language: python
        entry: ./echo.py
```

`echo.py`:

```python
# /// script
# requires-python = ">=3.13"
# dependencies = [
#     "pyecho-cli",
# ]
# ///

from pyecho import main
main()
```

**Important Notes:**

- The first part of the `entry` field must be a path to a local Python script
- If `additional_dependencies` is specified in `.pre-commit-config.yaml`, script metadata will be ignored
- When both `language_version` (in config) and `requires-python` (in script) are set, `language_version` takes precedence
- Only `dependencies` and `requires-python` fields are supported; other metadata like `tool.uv` is ignored

## Command line interface

### `prek run`

- `prek run [HOOK|PROJECT]...` supports selecting or skipping multiple projects or hooks in workspace mode. See [Running Specific Hooks or Projects](workspace.md#running-specific-hooks-or-projects) for details.
- `prek` provides dynamic completions of hook id.
- `prek run --last-commit` to run hooks on files changed by the last commit.
- `prek run --directory <DIR>` to run hooks on a specified directory.

### `prek list`

`prek list` command lists all available hooks, their ids, and descriptions. This provides a better overview of the configured hooks.

### `prek auto-update`

- `prek auto-update` updates all projects in the workspace to their latest revisions.
- `prek auto-update` checks updates for the same repository only once, speeding up the process in workspace mode.
- `prek auto-update` supports `--dry-run` option to preview the updates without applying them.

### `prek sample-config`

- `prek sample-config` command has a `--file` option to write the sample configuration to a specific file.

### `prek cache`

- `prek cache clean` to remove all cached data.
- `prek cache gc` to remove unused cached repositories, environments and toolchains.
- `prek cache dir` to show the cache directory.

`prek clean` and `prek gc` are also available but hidden, as `prek cache` is preferred.
