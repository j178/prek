# Priority-based parallel hook execution

This document outlines the design for parallel hook execution using explicit priority levels.

## Motivation

By default, `prek` executes hooks sequentially. While safe, this is inefficient for independent tasks (e.g., linting different languages). This proposal introduces per-hook priorities to allow safe, parallel execution of hooks.

## Configuration

### Hook Configuration: `priority`

A new optional field `priority` is added to the hook configuration.

```yaml
- id: cargo-fmt
  priority: 10
```

* **Type**: `u32`
* **Default**: `None` (auto-populated by hook index)

When `priority` is omitted, the scheduler assigns the hook a priority equal to its index in the configuration file, starting at `0`. This preserves the current sequential behavior by giving each hook a unique, increasing priority by default.

## Execution Model

Execution is driven purely by priority numbers:

### Scope

`priority` is **global within a single configuration file**. That is, priorities are compared across **all hooks in the same `.pre-commit-config.yaml`**, even if the hooks live under different `repos:` entries.

`priority` does **not** apply across *different* `.pre-commit-config.yaml` files (or separate `prek` runs with different configs). Each config file is scheduled independently.

1. **Ordering**: Hooks run from the lowest priority value to the highest.
2. **Concurrency**: Hooks that share the same priority execute concurrently, subject to the global concurrency limit (default: number of CPUs).
3. **Defaults**: Without explicit priorities, each hook receives a unique priority derived from its position, so execution remains sequential and backwards-compatible.
4. **Conflicts**: If two hooks intentionally share a priority, they will be run in parallel. Users are responsible for assigning priorities that match their desired grouping.

## `require_serial` Clarification

The existing `require_serial` configuration key often causes confusion. In this design, its meaning is strictly scoped:

* **`require_serial: true`**: Controls **invocation concurrency for that hook**. When running a hook against files, `prek` limits that hook to a single in-flight invocation at a time. This effectively disables running multiple batches of the *same hook* concurrently.
  * `prek` will still try to pass all files in one invocation, but may split into multiple invocations if the OS command-line length limit would be exceeded.
* **It does NOT imply exclusive execution**. A hook with `require_serial: true` can still run in parallel with other hooks that share its `priority`.
* If a hook *must* run alone (e.g., it modifies global state), it should be assigned a unique priority value that no other hook uses.

## Design Considerations

### Mixing Explicit and Implicit Priorities

Implicit priorities are always derived from the hook's position in the configuration (0-based), regardless of any explicitly configured priorities on other hooks.

Positions are taken from the **fully flattened hook list for the current `.pre-commit-config.yaml`**, in the order hooks appear as `repos:` and `hooks:` are read. In other words, implicit priorities are assigned across the whole file, not per-repo.

Example:

* Hook at index `0` with no `priority` gets implicit priority `0`.
* Hook at index `1` with `priority: 10` keeps priority `10`.
* Hook at index `2` with no `priority` gets implicit priority `2`.

This means a later hook with an implicit priority can run before an earlier hook that was assigned a larger explicit priority.

If you want to avoid surprises when introducing explicit priorities, prefer setting `priority` on all hooks (or at least on every hook whose relative order matters).

### Grouped Output

If files are modified during a *parallel priority group*, `prek` can only tell that **one or more hooks in the group** made changes (not which one). In this case, `prek` prints a grouped tree for the whole priority group and marks the group as failed.

Example:

```
  Files were modified by following hooks...................................Failed
    ┌ Modifies File........................................................Passed
    │ Prints Output........................................................Passed
    └ No Output............................................................Passed
  Later Hook...............................................................Passed
```

### Fail Fast

If `fail_fast` is enabled:

* If a hook fails, `prek` should wait for currently running hooks with the *current priority* to finish, but **abort** the execution of higher-priority groups.

### Example Configuration

```yaml
repos:
  - repo: local
    hooks:
      - id: cargo-fmt
        name: Format Rust
        entry: cargo fmt
        language: system
        priority: 0  # Runs first

  # These hooks are in different repos, but share the same priority,
  # so they can run concurrently.
  - repo: local
    hooks:
      - id: ruff
        name: Lint Python
        entry: ruff check
        language: system
        priority: 10

  - repo: local
    hooks:
      - id: shellcheck
        name: Lint Shell
        entry: shellcheck
        language: system
        priority: 10

  - repo: local
    hooks:
      - id: integration-tests
        name: Integration Tests
        entry: just test
        language: system
        priority: 20 # Starts after priority=10 group completes
```
