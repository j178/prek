# Compatibility with pre-commit

`prek` aims to be a practical drop-in replacement for `pre-commit` for existing repositories, hook configs, and day-to-day workflows.

## What works unchanged

- Existing `.pre-commit-config.yaml` and `.pre-commit-config.yml` files work in `prek`. See [Configuration](configuration.md).
- Most common `pre-commit` workflows keep working, including the usual hook repositories and manifests.
- Several upstream command spellings are still accepted as compatibility aliases, so existing scripts do not need to be rewritten immediately.

## Preferred command and flag spellings

`prek` keeps compatibility aliases for the commands below, but the preferred spellings use a more descriptive CLI layout.

| Compatibility spelling | Preferred `prek` spelling |
| -- | -- |
| `prek install-hooks` | `prek prepare-hooks` |
| `prek install --install-hooks` | `prek install --prepare-hooks` |
| `prek autoupdate` | `prek auto-update` |
| `prek gc` | `prek cache gc` |
| `prek clean` | `prek cache clean` |
| `prek init-templatedir` | `prek util init-template-dir` |
| `prek init-template-dir` | `prek util init-template-dir` |
| `pre-commit migrate-config` | Not provided directly; use `prek util yaml-to-toml` to migrate YAML to `prek.toml` |

## Why the CLI is reorganized

`pre-commit` keeps many maintenance commands as separate top-level entries. `prek` reorganizes some of them so the command tree is easier to navigate:

- related cache operations live under `prek cache`
- helper and migration commands live under `prek util`
- `prepare-hooks` describes what the command actually does more clearly than `install-hooks`

That improves discoverability without dropping compatibility, because the older spellings remain available.

## Not implemented

- `pre-commit hazmat` is not implemented in `prek`.

## If you need strict upstream portability

If the same config must continue working in upstream `pre-commit`, stay with the YAML config format and avoid `prek`-only features such as:

- `prek.toml`
- `repo: builtin`
- glob mappings for `files` and `exclude`
- workspace mode

See [Configuration](configuration.md) for config format guidance, [Configuration Reference](reference/configuration.md) for key-level details, and [Differences](diff.md) for broader behavior and CLI differences.
