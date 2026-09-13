# Built-in Hooks

prek includes fast, Rust-native implementations of popular hooks for speed and low overhead.

Use `repo: builtin` to select them directly, or keep a supported remote hook
config and let prek use its automatic fast path.

|  | `repo: builtin` | Automatic fast path |
| -- | -- | -- |
| Config remains usable by upstream `pre-commit` | No | Yes |
| Remote repository and manifest | Not used | Cloned at the pinned `rev` |
| Environment available for fallback | Not needed | Yes |
| Network needed for first preparation | No | Yes |
| How to opt out | Replace `repo: builtin` with a remote or local hook | Set the hook's declared language or `PREK_NO_FAST_PATH=1` |

!!! note "Check implementation notes when behavior matters"

    The Rust implementations target the same purpose as their upstream hooks,
    but a hook can have documented differences in arguments, defaults, or edge
    cases. Check its entry in the [Hook Reference](reference/built-in-hooks.md#hook-reference). To compare
    behavior, disable the fast path and run the pinned implementation.

## Use built-in hooks directly

Add `repo: builtin` and select hooks by ID:

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

## Use the automatic fast path

For supported hooks from `https://github.com/pre-commit/pre-commit-hooks`, prek
runs the built-in implementation automatically. Your existing remote config
can stay compatible with upstream pre-commit.
The `rev` field does not affect fast-path detection. It still selects the
manifest that prek reads and the repository implementation used for fallback.

```yaml
repos:
  - repo: https://github.com/pre-commit/pre-commit-hooks  # Enables fast path
    rev: v6.0.0  # Used for the manifest and fallback, not fast-path detection
    hooks:
      - id: trailing-whitespace
```

!!! note

    In this mode, `prek` will still clone the repository and create the environment (e.g., a Python venv) to ensure full compatibility and fallback capabilities. However, the actual hook execution bypasses the environment and runs the native Rust code.

See [fast-path support](reference/built-in-hooks.md#automatic-fast-path) for the list of
hooks that use the built-in implementation automatically. Other hooks run via
the standard method.

### Run the repository implementation

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
