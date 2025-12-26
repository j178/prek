# Configuration

Prek is **fully compatible** with the [pre-commit](https://pre-commit.com/) configuration file `.pre-commit-config.yaml`, so your existing configs work unchanged.

For the complete, authoritative schema and semantics, refer to the official pre-commit docs:
[pre-commit.com](https://pre-commit.com/)

The snippet below is a **concise overview** of commonly used keys (not an exhaustive schema):

```yaml
# Common top-level keys
default_language_version: {python: python3.12, ...}
default_stages: [commit, ...]
files: Regex to include
exclude: Regex to exclude
fail_fast: true|false
minimum_pre_commit_version: "X.Y.Z"

repos:
  - repo: https://example.com/some-repo | local | meta
    rev: v1.2.3
    hooks:
      - id: some-hook
        # Common hook keys
        name: Optional display name
        entry: Override command (varies by language)
        language: system | python | node | rust | ...
        args: ["--flag", "value"]
        files: Regex to include files
        exclude: Regex to exclude files
        types: [python, yaml, ...]
        types_or: ["..."]
        exclude_types: ["..."]
        stages: [commit, push, manual, ...]
        verbose: true|false
        pass_filenames: true|false
        always_run: true|false
        require_serial: true|false
        fail_fast: true|false
        additional_dependencies: ["pkg==1.2.3", ...]   # language-specific
        language_version: "python3.12"                 # language-specific
        log_file: path/to/log.txt
```

## Prek specific configurations

The following configuration keys are **prek-specific** and are **not supported by the original `pre-commit`** (at least at the time of writing).

If you run the same config with `pre-commit`, it may warn about **unexpected/unknown keys**.

### Project-level

#### `files` / `exclude` patterns

Patterns remain regex by default (matching pre-commit), but you can also opt into globs:

```yaml
repos:
  - repo: local
    hooks:
      - id: format-python
        entry: bash -c 'echo formatting "$@"' _
        language: system
        files:
          glob: src/**/*.py
        exclude:
          glob: src/vendor/**
```

Both forms work at the top-level config and per-hook.

#### `minimum_prek_version`

Specify the minimum required version of prek for the configuration. If the installed version is lower, prek will exit with an error.

Example:

  ```yaml
  minimum_prek_version: '0.2.0'
  ```

The original `minimum_pre_commit_version` option has no effect and gets ignored in prek.

#### `orphan`

!!! note

    `orphan` only applies in workspace mode with nested projects.

By default, files in subprojects are processed multiple times - once for each project in the hierarchy that contains them. Setting `orphan: true` isolates the project from parent configurations, ensuring files in this project are processed only by this project and not by any parent projects.

Example:

  ```yaml
  orphan: true
  repos:
    - repo: https://github.com/astral-sh/ruff-pre-commit
      rev: v0.8.4
      hooks:
        - id: ruff
  ```

For more details and examples, see [Workspace Mode - File Processing Behavior](workspace.md#file-processing-behavior).

### Hook-level

#### `priority`

Each hook can set an explicit `priority` (a non-negative integer) that controls when it runs and with which hooks it may execute in parallel.

Scope:

- `priority` is evaluated **within a single `.pre-commit-config.yaml`** and is compared across **all hooks in that file**, even if they appear under different `repos:` entries.
- `priority` does **not** coordinate across different `.pre-commit-config.yaml` files. In workspace mode (nested projects), each project’s config file is scheduled independently.

Hooks run in ascending priority order: **lower `priority` values run earlier**. Hooks that share the same `priority` value run concurrently, subject to the global concurrency limit (defaults to the number of CPU cores; set `PREK_NO_CONCURRENCY=1` to force concurrency to `1`).

When `priority` is omitted, prek automatically assigns the hook a value equal to its index in the configuration file, preserving the original sequential behavior.

Example:

```yaml
repos:
  - repo: local
    hooks:
      - id: format
        name: Format
        language: system
        entry: python3 -m ruff format
        always_run: true
        priority: 0       # runs first

      # No explicit priority: automatically assigned based on position.
      # In this example it is the next hook in the file, so it gets priority=1.
      - id: lint-sh
        name: Shell Lint
        language: system
        entry: shellcheck
        always_run: true

  # These two hooks are defined under different repos, but share the same
  # priority value, so they can run concurrently.
  - repo: https://github.com/astral-sh/ruff-pre-commit
    rev: v0.8.4
    hooks:
      - id: ruff
        name: Python Lint
        args: [--fix]
        always_run: true
        priority: 10

  - repo: local
    hooks:
      - id: lint-py
        name: Python Lint (alt)
        language: system
        entry: python3 -m ruff check
        always_run: true
        priority: 10

  - repo: local
    hooks:
      - id: tests
        name: Integration Tests
        language: system
        entry: just test
        always_run: true
        priority: 20      # starts after the priority=10 group completes
```

If a hook must be completely isolated, give it a unique priority value so no other hook can join its group.

!!! danger "Parallel hooks modifying files"

    Running hooks in parallel is powerful, but it is **your responsibility** to group hooks safely.

    If two hooks run in the same priority group and they modify the same files (or otherwise depend on shared state), the result is **undefined** — files may be corrupted.

    If hooks must not overlap, assign them different priorities (or give the sensitive hook a unique priority).

!!! note "`require_serial`"

    `require_serial: true` limits that hook to a single in-flight invocation at a time (so it won’t run multiple batches concurrently).

    It may still split into multiple invocations if the OS command-line length limit would be exceeded.

    It does **not** make the hook run exclusively; use a unique `priority` for exclusive execution.

## Environment variables

Prek supports the following environment variables:

- `PREK_HOME` — Override the prek data directory (caches, toolchains, hook envs). Defaults to `~/.cache/prek` on macOS and Linux, and `%LOCALAPPDATA%\prek` on Windows.
- `PREK_COLOR` — Control colored output: auto (default), always, or never.
- `PREK_SKIP` — Comma-separated list of hook IDs to skip (e.g. black,ruff). See [Skipping Projects or Hooks](workspace.md#skipping-projects-or-hooks) for details.
- `PREK_ALLOW_NO_CONFIG` — Allow running without a .pre-commit-config.yaml (useful for ad‑hoc runs).
- `PREK_NO_CONCURRENCY` — Disable parallelism for installs and runs (set `PREK_NO_CONCURRENCY=1` to force concurrency to `1`).
- `PREK_NO_FAST_PATH` — Disable Rust-native built-in hooks; always use the original hook implementation. See [Built-in Fast Hooks](builtin.md) for details.

- `PREK_UV_SOURCE` — Control how uv (Python package installer) is installed. Options:

    - `github` (download from GitHub releases)
    - `pypi` (install from PyPI)
    - `tuna` (use Tsinghua University mirror)
    - `aliyun` (use Alibaba Cloud mirror)
    - `tencent` (use Tencent Cloud mirror)
    - `pip` (install via pip)
    - a custom PyPI mirror URL

    If not set, prek automatically selects the best available source.

- `PREK_NATIVE_TLS` - Use system's trusted store instead of the bundled `webpki-roots` crate.

- `PREK_CONTAINER_RUNTIME` - Specify the container runtime to use for container-based hooks (e.g., `docker`, `docker_image`). Options:

    - `auto` (default, auto-detect available runtime)
    - `docker`
    - `podman`

Compatibility fallbacks:

- `PRE_COMMIT_ALLOW_NO_CONFIG` — Fallback for `PREK_ALLOW_NO_CONFIG`.
- `PRE_COMMIT_NO_CONCURRENCY` — Fallback for `PREK_NO_CONCURRENCY`.
- `SKIP` — Fallback for `PREK_SKIP`.
