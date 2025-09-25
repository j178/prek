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

- `prek` supports Python toolchain management, it will install the required Python versions automatically.
- `prek` uses `uv` for creating virtual environments and installing dependencies.
- `prek` supports Python hooks with PEP 723 inline metadata.

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

### `prek sample-config`

- `prek sample-config` command has a `--file` option to write the sample configuration to a specific file.

### `prek cache`

- `prek cache clean` to remove all cached data.
- `prek cache gc` to remove unused cached repositories, environments and toolchains.
- `prek cache dir` to show the cache directory.

`prek clean` and `prek gc` are also available but hidden, as `prek cache` is preferred.
