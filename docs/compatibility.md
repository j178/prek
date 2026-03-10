# Compatibility with pre-commit

`prek` is designed to be a practical drop-in replacement for `pre-commit`.

- Existing `.pre-commit-config.yaml` and `.pre-commit-config.yml` files work unchanged. See [Configuration](configuration.md).
- Most day-to-day `pre-commit` commands work unchanged in `prek`.

## Command and flag differences

Only the commands and flags below differ from the preferred `prek` spelling. The compatibility forms are still accepted so existing scripts do not break.

- `prek install-hooks` still works, but `prek prepare-hooks` is the preferred spelling.
- `prek install --install-hooks` still works, but `prek install --prepare-hooks` is the preferred flag spelling.
- `prek autoupdate` still works, but `prek auto-update` is the preferred spelling.
- `prek gc` still works as a hidden compatibility command, but `prek cache gc` is preferred.
- `prek clean` still works as a hidden compatibility command, but `prek cache clean` is preferred.
- `prek init-templatedir` and `prek init-template-dir` still work as hidden compatibility commands, but `prek util init-template-dir` is preferred.
- `pre-commit hazmat` is not implemented in `prek`.
- `pre-commit migrate-config` is not provided as a direct command. Use `prek util yaml-to-toml` if you want to migrate from YAML to `prek.toml`.

If you need strict upstream portability, stay with the YAML config format and avoid `prek`-only features such as `prek.toml`, `repo: builtin`, glob mappings for `files` and `exclude`, and workspace mode. See [Configuration](configuration.md) and [Differences](diff.md).
