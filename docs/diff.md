# Differences from pre-commit

## General differences

- `prek` supports `.pre-commit-config.yaml`, `.pre-commit-config.yml`, and native `prek.toml` configuration files. Use [`prek util yaml-to-toml`](cli.md#prek-util-yaml-to-toml) to convert an existing YAML config.
- `prek` implements some common hooks from `pre-commit-hooks` in Rust for better performance.
- `prek` supports `repo: builtin` for offline, zero-setup hooks.
- `prek` uses `~/.cache/prek` as the default cache directory for repos, environments and toolchains.
- `prek` decouples hook environments from their repositories, allowing shared toolchains and environments across hooks.
- `prek` supports `language_version` as a semver specifier and automatically installs the required toolchains.
- `prek` supports `files` and `exclude` as glob lists (in addition to regex) via `glob` mappings. See [Configuration](configuration.md#top-level-files).
- `prek` reports more precise configuration parsing errors, including exact source locations.

## Workspace mode

`prek` supports workspace mode, allowing you to run hooks for multiple projects in a single command. Each subproject can keep its own `prek.toml` or `.pre-commit-config.yaml` file.

See [Workspace Mode](./workspace.md) for more information.

## Language support

See the dedicated [Language Support](languages.md) page for a complete list of supported languages, prek-specific behavior, and unsupported languages.

Recent releases added support for more managed hook runtimes, including Bun, Julia, Deno, and experimental .NET support.

## Command line interface

For a compatibility-focused command mapping, see [Compatibility with pre-commit](compatibility.md).

### `prek run`

- `prek run [HOOK|PROJECT]...` supports selecting or skipping multiple projects or hooks in workspace mode, instead of only accepting a single optional hook id. See [Running Specific Hooks or Projects](workspace.md#running-specific-hooks-or-projects) for details.
- `prek run` can execute hooks in parallel by priority (hooks with the same [`priority`](./configuration.md#priority) may run concurrently), instead of strictly serial execution.
- `prek` provides dynamic completion for hook ids.
- `prek run --dry-run` shows which hooks would run without executing them.
- `prek run --last-commit` runs hooks on files changed by the last commit.
- `prek run --directory <DIR>` runs hooks on a specified directory.
- `prek run --no-fail-fast` lets you override the configured `fail_fast` setting for a single run and continue after failures.

### `prek install`

- `prek install` and `prek uninstall` honor repo-local and worktree-local `core.hooksPath` when choosing where to manage Git shims.

### `prek validate-config`

- `prek validate-config` accepts both `prek.toml` and `.pre-commit-config.yaml`.

### `prek list`

`prek list` lists all available hooks, their ids, and descriptions. This provides a better overview of the configured hooks.

### `prek auto-update`

- `prek auto-update` updates all projects in the workspace to their latest revisions.
- `prek auto-update` checks updates for the same repository only once, speeding up the process in workspace mode.
- `prek auto-update` supports `--dry-run` to preview the updates without applying them.
- `prek auto-update` supports `--exit-code` to exit non-zero when updates are available, and `--check` as an alias for `--dry-run --exit-code`.
- `prek auto-update` validates pinned SHA revisions against fetched upstream refs, including impostor-commit detection, and keeps stale `# frozen:` comments in sync when it can.
- `prek auto-update` supports the `--cooldown-days` option to skip releases newer than the specified number of days (based on the tag creation timestamp for annotated tags, or the tagged commit timestamp for lightweight tags).
- `prek auto-update` supports `--exclude-repo` to skip selected repositories while updating everything else.
- `prek auto-update` supports tag filtering with `--include-tag`, `--exclude-tag`, `--repo-include-tag`, and `--repo-exclude-tag`, using glob patterns to keep or remove matching tags before selecting an update.

### `prek sample-config`

- `prek sample-config` can generate either YAML or TOML and can write directly to a file with `--file`.

### `prek util`

- `prek util identify` shows the file-identification tags prek uses for filtering and debugging hook selection.
- `prek util list-builtins` lists all built-in hooks bundled with prek.
- `prek util yaml-to-toml` converts `.pre-commit-config.yaml` to `prek.toml`.

### `prek cache`

- `prek` groups cache maintenance under `prek cache` instead of separate top-level `clean` and `gc` commands.
- `prek cache gc` removes unused cached repositories, environments and toolchains, and supports `--dry-run`.
- `prek cache clean` removes all cached data.
- `prek cache dir` and `prek cache size` help inspect the cache before or after cleanup.

## Not implemented

The `pre-commit hazmat` subcommand introduced in pre-commit
[v4.5.0](https://github.com/pre-commit/pre-commit/releases/tag/v4.5.0) is not
implemented. This command is niche and unlikely to be widely used.
