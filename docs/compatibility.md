# Compatibility with pre-commit

`prek` is a practical drop-in replacement for `pre-commit`, with support for its repositories, configuration files, hooks, and command-line workflows.

## What works unchanged

- Existing `.pre-commit-config.yaml` and `.pre-commit-config.yml` files work in `prek`. See [Configuration](configuration.md).
- Existing hook repositories and `.pre-commit-hooks.yaml` manifests work in `prek`.
- The main user-facing subcommands keep their upstream names: `install`, `run`, `sample-config`, `try-repo`, `uninstall`, `validate-config`, and `validate-manifest`.

## Command compatibility

For a smaller and more consistent command tree, `prek` hides some compatibility spellings from `prek --help` and groups some operations under `cache` or `util`. Hidden commands are still callable; hiding only keeps them out of the help output.

| `pre-commit` spelling | Preferred `prek` spelling | Availability in `prek` |
| -- | -- | -- |
| `pre-commit install-hooks` | `prek prepare-hooks` | `prek install-hooks` remains available as a hidden alias. |
| `pre-commit install --install-hooks` | `prek install --prepare-hooks` | `--install-hooks` remains available as a hidden alias. |
| `pre-commit autoupdate` | `prek update` | `prek autoupdate` remains available as a hidden alias. |
| `pre-commit gc` | `prek cache gc` | `prek gc` remains available but is hidden. |
| `pre-commit clean` | `prek cache clean` | `prek clean` remains available but is hidden. |
| `pre-commit init-templatedir` | `prek util init-template-dir` | `prek init-templatedir` remains available but is hidden. |
| `pre-commit migrate-config` | `prek util yaml-to-toml` | Configuration migration lives under `prek util` and targets the native `prek.toml` format. |
| `pre-commit help <command>` | `prek <command> --help` | Help uses the standard `--help` flag instead of a separate subcommand. |
| `pre-commit hook-impl` | `prek hook-impl` | Available but hidden because installed Git hooks invoke it internally. |

## Why the CLI is reorganized

`pre-commit` keeps many maintenance commands as separate top-level entries. `prek` reorganizes some of them so the command tree is easier to navigate:

- related cache operations live under `prek cache`
- helper and migration commands live under `prek util`
- `prepare-hooks` describes what the command actually does more clearly than `install-hooks`

This organization keeps the primary help output focused without removing the underlying functionality or compatibility entry points.

## Not implemented

- `pre-commit hazmat` is not implemented in `prek`.

## If you need strict upstream portability

If the same config must continue working in upstream `pre-commit`, stay with the YAML config format and avoid `prek`-only features such as:

- `prek.toml`
- `repo: builtin`
- glob mappings for `files` and `exclude`
- workspace mode

See [Configuration](configuration.md) for config format guidance, [Configuration Reference](reference/configuration.md) for key-level details, and [Differences](diff.md) for broader behavior and CLI differences.
