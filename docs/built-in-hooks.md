# Built-in Hooks

prek includes fast, Rust-native implementations of popular hooks for speed and low overhead. These hooks are bundled directly into the `prek` binary, eliminating the need for external interpreters like Python for these specific checks.

Built-in hooks come into play in two ways:

1. **Automatic Fast Path**: Automatically replacing execution for known remote repositories.
2. **Explicit Builtin Repository**: Using `repo: builtin` for offline, zero-setup hooks.

|  | Automatic fast path | `repo: builtin` |
| -- | -- | -- |
| Config remains usable by upstream `pre-commit` | Yes | No |
| Remote repository and manifest | Cloned at the pinned `rev` | Not used |
| Environment available for fallback | Yes | Not needed |
| Network needed for first preparation | Yes | No |
| How to opt out | Set the hook's declared language or `PREK_NO_FAST_PATH=1` | Replace `repo: builtin` with a remote or local hook |

!!! note "Check implementation notes when behavior matters"

    The Rust implementations target the same purpose as their upstream hooks,
    but a hook can have documented differences in arguments, defaults, or edge
    cases. Check its entry in the [Hook Reference](reference/built-in-hooks.md#hook-reference). To compare
    behavior, disable the fast path and run the pinned implementation.

## 1. Automatic Fast Path

When you use a standard configuration pointing to a supported repository (like `https://github.com/pre-commit/pre-commit-hooks`), `prek` automatically detects this and runs its internal Rust implementation instead of the Python version defined in the repository.

The fast path is activated when the `repo` URL matches `https://github.com/pre-commit/pre-commit-hooks`. No need to change anything in your configuration.
The `rev` field does not affect fast-path detection. It still selects the
manifest that prek reads and the repository implementation used for fallback.

This provides a speed boost while keeping your configuration compatible with the original `pre-commit` tool.

```yaml
repos:
  - repo: https://github.com/pre-commit/pre-commit-hooks  # Enables fast path
    rev: v4.5.0  # Used for the manifest and fallback, not fast-path detection
    hooks:
      - id: trailing-whitespace
```

!!! note

    In this mode, `prek` will still clone the repository and create the environment (e.g., a Python venv) to ensure full compatibility and fallback capabilities. However, the actual hook execution bypasses the environment and runs the native Rust code.

See [fast-path support](reference/built-in-hooks.md#automatic-fast-path) for the list of
hooks that use the built-in implementation automatically. Other hooks run via
the standard method.

### Disabling the fast path

To use the pinned repository implementation for a single hook, explicitly set the language
declared by that hook:

```yaml
repos:
  - repo: https://github.com/pre-commit/pre-commit-hooks
    rev: v6.0.0
    hooks:
      - id: check-yaml
        language: python  # Use the pinned repository implementation
```

To disable the fast path for every hook in a prek invocation:

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

=== "prek.toml"

    ```toml
    [[repos]]
    repo = "builtin"
    hooks = [
      { id = "trailing-whitespace" },
      { id = "check-added-large-files" },
    ]
    ```

=== ".pre-commit-config.yaml"

    ```yaml
    repos:
      - repo: builtin
        hooks:
          - id: trailing-whitespace
          - id: check-added-large-files
    ```

List the builtins bundled with your installed prek version using:

```bash
prek util list-builtins
```

See the [built-in hook reference](reference/built-in-hooks.md) for the complete list of
hooks, supported arguments, and behavior notes.
