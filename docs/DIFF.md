## Difference from pre-commit

- `prek` supports both `.pre-commit-config.yaml` and `.pre-commit-config.yml` configuration files.
- `prek` implements some common hooks from `pre-commit-hooks` in Rust for better performance.
- `prek` uses `~/.prek` as the default cache directory for repos, environments and toolchains.
- `prek` decoupled hook environment from their repositories, allowing shared toolchains and environments across hooks.
- `prek` supports `language-version` as a semver specifier and automatically installs the required toolchains.
- `prek` provides dynamic completions of hook id.

### `prek run`

- `prek run --last-commit` to run hooks on files changed by the last commit.
- `prek run --directory <DIR>` to run hooks on a specified directory.
- `prek run [HOOK_ID] [HOOK_ID] ...` to run multiple hooks.

### `prek list`

`prek list` command lists all available hooks, their ids, and descriptions. This provides a better overview of the configured hooks.

### `prek sample-config`

- `prek sample-config` command has a `--file` option to write the sample configuration to a specific file.

### Python support

- `prek` supports Python toolchain management, it will install the required Python versions automatically.
- `prek` uses `uv` for creating virtual environments and installing dependencies.
- `prek` supports Python hooks with PEP 723 inline metadata.

### Future plans

- Built-in support for monorepos.
- Global configurations.
