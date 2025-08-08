## Difference from pre-commit

- `prek` supports both `.pre-commit-config.yaml` and `.pre-commit-config.yml` configuration files.
- `prek` implements some common hooks from `pre-commit-hooks` in Rust for better performance.
- `prek` uses `~/.prek` as the default cache directory for repos, environments and toolchains.
- `prek` decoupled hook environment from their repositories, allowing shared toolchains and environments across hooks.
- `prek` supports Python toolchain management, it delegates to `uv`, and uses `uv` for virtual environments and dependencies.
- `prek` supports `language-version` as a semver specifier and automatically installs the required toolchains.
- `prek sample-config` command has a `--file` option to write the sample configuration to a specific file.

### Future plans

- Built-in support for monorepos.
- Global configurations.
