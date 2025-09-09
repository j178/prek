# Built-in Fast Hooks

Prek includes fast, Rust-native implementations of popular hooks for speed and low overhead. When a matching hook from a popular repository (for example, `pre-commit/pre-commit-hooks`) is detected, Prek can run an internal implementation instead of spawning external interpreters.

Currently, only `pre-commit/pre-commit-hooks` is implemented. More popular repositories may be added over time.

## Currently implemented hooks

### <https://github.com/pre-commit/pre-commit-hooks>

- `trailing-whitespace` (Trim trailing whitespace)
- `check-added-large-files` (Prevent committing large files)
- `end-of-file-fixer` (Ensure newline at EOF)
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
