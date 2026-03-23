# DAG-based hook scheduling with `group` and `after`

This document proposes two new optional configuration fields — `group` and `after` — that let users express a DAG of hook dependencies. prek builds the DAG, schedules maximally parallel execution, and only serializes where explicit edges exist.

## Motivation

The existing `priority: u32` field conflates two concerns: execution ordering and concurrency grouping. Hooks with the same priority run in parallel; different priorities run sequentially. This forces global coordination between hooks that operate on completely unrelated file types (e.g., Python formatters block Markdown linters). It also requires maintaining fragile numeric schemes that are hard to review and easy to break when adding hooks.

## Configuration

### `group: string`

An optional label on a hook. No ordering implications by itself. Purely a reference handle so other hooks can depend on the entire group completing.

```yaml
- id: ruff-format
  group: formatters
```

### `after: list[string]`

An optional list of hook IDs or `group:<name>` references. The hook waits for all listed dependencies to complete before running.

- `after: [ruff-lint]` — wait for the hook with id `ruff-lint`
- `after: [group:formatters]` — wait for every hook in the `formatters` group to finish

```yaml
- id: mypy
  after: [group:formatters]
```

## Semantics

1. **No `after`** = run immediately, in parallel with everything else.
2. **`after` creates edges in a DAG**; prek topologically sorts and runs maximally parallel.
3. **`group` is just a label**, no implicit ordering within a group.
4. **`after: [group:X]`** means after ALL hooks in group X complete.
5. **Cycles are a config error.**

## Backwards Compatibility

- `priority` continues to work exactly as before.
- `group`/`after` and `priority` are **mutually exclusive on the same hook** (error if both specified).
- If no hooks in a project use `group`/`after`, the existing priority-group scheduler is used (fully backwards compatible).
- If any hook in a project uses `group`/`after`, **all** hooks in that project are scheduled via the DAG. Hooks without `after` have zero dependencies and run in the first wave (maximally parallel).

## Execution Model

### DAG Resolution

1. Build lookup maps: hook id → index, group name → list of indices.
2. Resolve `after` references: plain IDs map to single edges; `group:<name>` references expand to edges from all hooks in that group.
3. Run Kahn's algorithm to detect cycles; report an error with the involved hook IDs if a cycle is found.

### Scheduling

The scheduler maintains:

- A set of "ready" hooks (all deps satisfied, not yet completed).
- A set of completed hooks.

Each iteration:

1. Collect all ready hooks.
2. Run them concurrently (respecting the global concurrency limit).
3. As hooks complete, decrement dependency counts for their dependents.
4. Newly-ready hooks are collected in the next iteration.

This naturally achieves maximal parallelism — hooks run as soon as their dependencies are satisfied.

### Fail Fast

If `fail_fast` is enabled and a hook in a DAG wave fails, prek aborts after the current wave completes (same behavior as priority groups).

## Example Configuration

```yaml
repos:
  - repo: local
    hooks:
      - id: ruff-format
        name: Format Python
        entry: ruff format
        language: system
        group: formatters

      - id: cargo-fmt
        name: Format Rust
        entry: cargo fmt
        language: system
        group: formatters

      - id: prettier
        name: Format Markdown
        entry: prettier --write
        language: system
        group: formatters

      - id: ruff-lint
        name: Lint Python
        entry: ruff check
        language: system
        after: [ruff-format]

      - id: clippy
        name: Lint Rust
        entry: cargo clippy
        language: system
        after: [cargo-fmt]

      - id: integration-tests
        name: Integration Tests
        entry: just test
        language: system
        after: [group:formatters]
```

In this configuration:

- All three formatters run in parallel (no dependencies).
- `ruff-lint` waits only for `ruff-format`; `clippy` waits only for `cargo-fmt`.
- `integration-tests` waits for all formatters to complete.
- `ruff-lint` and `clippy` can run in parallel with each other and with `integration-tests` (once their respective dependencies are met).

## Error Cases

| Condition | Error |
| -- | -- |
| `priority` and `group`/`after` on same hook | "`priority` and `group`/`after` are mutually exclusive on the same hook" |
| `after: [nonexistent-id]` | "Hook `X` has `after: [nonexistent-id]` but no hook with id `nonexistent-id` exists" |
| `after: [group:missing]` | "Hook `X` has `after: [group:missing]` but no hook belongs to group `missing`" |
| Cycle in dependencies | "Cycle detected in hook dependencies: hook-a, hook-b" |
