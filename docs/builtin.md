# Built-in Fast Hooks

Prek includes fast, Rust-native implementations of popular hooks for speed and low overhead. When a matching hook from a popular repository (for example, `pre-commit/pre-commit-hooks`) is detected, prek can run an internal implementation instead of spawning external interpreters.

Built-in hooks are activated when the `repo` field in your configuration exactly matches a supported repository URL, regardless of the `rev` field. For example:

```yaml
repos:
  - repo: https://github.com/pre-commit/pre-commit-hooks  # Enables fast path
    rev: v4.5.0  # This is ignored for fast path detection
    hooks:
      - id: trailing-whitespace
```

**Note**: Even when all hooks in a repository are supported as built-in hooks, the repository will still be cloned and the environment will still be created as usual. Only the hook execution itself is replaced with the built-in implementation when running the hook. This may change in the future to skip cloning and environment setup when all hooks are built-in.

Currently, only `https://github.com/pre-commit/pre-commit-hooks` is supported. More popular repositories may be added over time.

## Currently implemented hooks

### <https://github.com/pre-commit/pre-commit-hooks>

- `trailing-whitespace` (Trim trailing whitespace)
- `check-added-large-files` (Prevent committing large files)
- `end-of-file-fixer` (Ensure newline at EOF)
- `fix-byte-order-marker` (Remove UTF-8 byte order marker)
- `check-json` (Validate JSON files)
- `check-toml` (Validate TOML files)
- `check-yaml` (Validate YAML files)
- `mixed-line-ending` (Normalize or check line endings)

Notes:

- `check-yaml` fast path does not yet support `--unsafe` or `--allow-multiple-documents` flags; for those cases, fast path is skipped automatically.
- Fast-path detection currently matches only the repository URL (e.g., `https://github.com/pre-commit/pre-commit-hooks`) and does not take the `rev` into account.

## Disabling the fast path

If you need to compare with the original behavior or encounter differences:

```bash
PREK_NO_FAST_PATH=1 prek run
```

This forces prek to fall back to the standard execution path.
