# Configuration

Prek is fully compatible with pre-commit configuration file `.pre-commit-config.yaml`, for example:

```yaml
repos:
  - repo: https://github.com/pre-commit/pre-commit-hooks
    rev: v6.0.0
    hooks:
      - id: trailing-whitespace
      - id: end-of-file-fixer

  - repo: https://github.com/crate-ci/typos
    rev: v1.36.2
    hooks:
      - id: typos
```

Your existing configs work unchanged with prek.

For configuration details, refer to the official pre-commit docs:
[pre-commit.com](https://pre-commit.com/)

## Prek specific configurations

### `minimum_prek_version`

Specify the minimum required version of prek for the configuration. If the installed version is lower, prek will exit with an error.

Example:

  ```yaml
  minimum_prek_version: '0.2.0'
  ```

The original `minimum_pre_commit_version` option has no effect and gets ignored in prek.

## Environment variables

Prek supports the following environment variables:

- `PREK_HOME` — Override the prek data directory (caches, toolchains, hook envs). Defaults to `~/.cache/prek` on macOS and Linux, and `%LOCALAPPDATA%\prek` on Windows.
- `PREK_COLOR` — Control colored output: auto (default), always, or never.
- `PREK_SKIP` — Comma-separated list of hook IDs to skip (e.g. black,ruff). See [Skipping Projects or Hooks](workspace.md#skipping-projects-or-hooks) for details.
- `PREK_ALLOW_NO_CONFIG` — Allow running without a .pre-commit-config.yaml (useful for ad‑hoc runs).
- `PREK_NO_CONCURRENCY` — Disable parallelism for installs and runs.
- `PREK_NO_FAST_PATH` — Disable Rust-native built-in hooks; always use the original hook implementation. See [Built-in Fast Hooks](builtin.md) for details.
- `PREK_UV_SOURCE` — Control how uv (Python package installer) is installed. Options: `github` (download from GitHub releases), `pypi` (install from PyPI), `tuna` (use Tsinghua University mirror), `aliyun` (use Alibaba Cloud mirror), `tencent` (use Tencent Cloud mirror), `pip` (install via pip), or a custom PyPI mirror URL. If not set, prek automatically selects the best available source.

Compatibility fallbacks:

- `PRE_COMMIT_ALLOW_NO_CONFIG` — Fallback for `PREK_ALLOW_NO_CONFIG`.
- `PRE_COMMIT_NO_CONCURRENCY` — Fallback for `PREK_NO_CONCURRENCY`.
- `SKIP` — Fallback for `PREK_SKIP`.
