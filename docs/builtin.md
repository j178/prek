# Built-in Fast Hooks

Prek includes fast, Rust-native implementations of popular hooks for speed and low overhead. These hooks are bundled directly into the `prek` binary, eliminating the need for external interpreters like Python for these specific checks.

Built-in hooks come into play in two ways:

1. **Automatic Fast Path**: Automatically replacing execution for known remote repositories.
2. **Explicit Builtin Repository**: Using `repo: builtin` for offline, zero-setup hooks.

## 1. Automatic Fast Path

When you use a standard configuration pointing to a supported repository (like `https://github.com/pre-commit/pre-commit-hooks`), `prek` automatically detects this and runs its internal Rust implementation instead of the Python version defined in the repository.

The fast path is activated when the `repo` URL matches `https://github.com/pre-commit/pre-commit-hooks`. No need to change anything in your configuration.
Note that the `rev` field is ignored for detection purposes.

This provides a speed boost while keeping your configuration compatible with the original `pre-commit` tool.

```yaml
repos:
  - repo: https://github.com/pre-commit/pre-commit-hooks  # Enables fast path
    rev: v4.5.0  # This is ignored for fast path detection
    hooks:
      - id: trailing-whitespace
```

!!! note
    In this mode, `prek` will still clone the repository and create the environment (e.g., a Python venv) to ensure full compatibility and fallback capabilities. However, the actual hook execution bypasses the environment and runs the native Rust code.

### Supported Hooks

Currently, only part of hooks from `https://github.com/pre-commit/pre-commit-hooks` is supported. More popular repositories may be added over time.

### <https://github.com/pre-commit/pre-commit-hooks>

- `trailing-whitespace` (Trim trailing whitespace)
- `check-added-large-files` (Prevent committing large files)
- `end-of-file-fixer` (Ensure newline at EOF)
- `fix-byte-order-marker` (Remove UTF-8 byte order marker)
- `check-json` (Validate JSON files)
- `check-toml` (Validate TOML files)
- `check-yaml` (Validate YAML files)
- `check-xml` (Validate XML files)
- `mixed-line-ending` (Normalize or check line endings)
- `check-symlinks` (Check for broken symlinks)
- `check-merge-conflict` (Check for merge conflicts)
- `detect-private-key` (Detect private keys)
- `no-commit-to-branch` (Prevent committing to protected branches)
- `check-executables-have-shebangs` (Ensures that (non-binary) executables have a shebang)

#### Notes

- `check-yaml` fast path does not yet support the `--unsafe` flag; for those cases, the automatic fast path is skipped.
- Other hooks from the repository will run using the standard pre-commit behavior.

### Disabling the fast path

If you need to compare with the original behavior or encounter differences:

```bash
PREK_NO_FAST_PATH=1 prek run
```

This forces prek to fall back to the standard execution path.

## 2. Explicit Builtin Repository

You can explicitly tell `prek` to use its internal hooks by setting `repo: builtin`.

This mode has significant benefits:

- **No network required**: Does not clone any repository.
- **No environment setup**: Does not create Python environments or install dependencies.
- **Maximum speed**: Instant startup and execution.

**Note**: Configurations using `repo: builtin` are **not compatible** with the standard `pre-commit` tool.

```yaml
repos:
  - repo: builtin
    hooks:
      - id: trailing-whitespace
      - id: check-added-large-files
```

### Supported Hooks

For `repo: builtin`, the following hooks are supported:

- `trailing-whitespace` (Trim trailing whitespace)
- `check-added-large-files` (Prevent committing large files)
- `end-of-file-fixer` (Ensure newline at EOF)
- `fix-byte-order-marker` (Remove UTF-8 byte order marker)
- `check-json` (Validate JSON files)
- `check-toml` (Validate TOML files)
- `check-yaml` (Validate YAML files)
- `check-xml` (Validate XML files)
- `mixed-line-ending` (Normalize or check line endings)
- `check-symlinks` (Check for broken symlinks)
- `check-merge-conflict` (Check for merge conflicts)
- `detect-private-key` (Detect private keys)
- `no-commit-to-branch` (Prevent committing to protected branches)
- `check-executables-have-shebangs` (Ensures that (non-binary) executables have a shebang)
