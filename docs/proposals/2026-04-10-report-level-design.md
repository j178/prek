# Design: `--report-level` flag for `prek run`

**Issue:** [#1777 — Proposal: Hook status level report](https://github.com/j178/prek/issues/1777)

**Date:** 2026-04-10

## Summary

Add a `--report-level <level>` flag to `prek run` that controls which hook statuses are displayed in output. This unifies three related requests (#1240, #1468, #1537) into a single output-filtering model. Hook execution, exit codes, and failure semantics are unchanged — this is purely a display filter.

## Report levels

Six levels, ordered from least to most verbose:

| Level | Shows |
| -- | -- |
| `silent` | No per-hook status lines |
| `fail` | Failed hooks only |
| `skipped-no-files` | Failed + hooks skipped because no files matched + unimplemented language hooks |
| `skipped` | All of the above + hooks excluded by `--skip`/`SKIP`/`PREK_SKIP` |
| `passed` | All of the above + passed hooks + dry-run hooks |
| `all` | Every hook status, including any future report-only states |

Each level is a threshold — a status is displayed if the report level is high enough to include it.

### Status-to-level mapping

| `RunStatus` | Minimum level to display |
| -- | -- |
| `Failed` | `fail` |
| `NoFiles` | `skipped-no-files` |
| `Unimplemented` | `skipped-no-files` |
| `Skipped` (new variant) | `skipped` |
| `Success` | `passed` |
| `DryRun` | `passed` |

**Default level:** `passed`, as recommended in the issue. This matches current behavior since all currently-displayed statuses (`Failed`, `NoFiles`, `Unimplemented`, `Success`, `DryRun`) are at or below `passed`. The only new visibility comes when a user explicitly sets `skipped` or higher — then selector-excluded hooks appear.

## Architecture: Two-pass approach

The implementation separates "what to run" from "what to show." Execution logic is untouched; the report level only affects rendering.

### Pass 1: Execution (unchanged, plus tracking)

The existing selector filtering at the top of `run()` is changed from a filter to a partition:

```
hooks.partition(|h| selectors.matches_hook(h)) -> (selected_hooks, skipped_hooks)
```

- `selected_hooks` enter the existing execution pipeline exactly as today
- `skipped_hooks` are never executed, never affect exit codes, never affect `fail_fast`
- `skipped_hooks` are passed into `run_hooks()` purely for display

### Pass 2: Rendering (filtered by report level)

`report_level` is threaded from CLI args through `run()` -> `run_hooks()` -> `render_priority_group()`.

A new method `ReportLevel::should_show(status: RunStatus) -> bool` gates each `status_printer.write()` call. If a status line is hidden, its associated verbose output (hook id, duration, exit code, output) is also hidden.

#### `silent` level specifics

When report level is `silent`:

- No per-hook status lines are printed
- Project headers (`Running hooks for X:`) are also suppressed
- Exit codes, the "files were modified" diff, and the "unimplemented languages" warning still appear

#### Group UI handling

The `group_modified_files` box UI (`┌│└` decorations) only renders if at least one hook in the group is visible at the current report level.

### Selector-skipped hook display

Selector-skipped hooks are rendered **after** all executed hooks within their project section, not interleaved by priority/index. This is a deliberate simplification — perfect interleaving would add complexity for minimal user benefit since skipped hooks have no output or timing. The project grouping provides enough context.

Display format for selector-skipped hooks:

```
my-hook..........................................(excluded by skip)Skipped
```

The suffix `(excluded by skip)` distinguishes from `(no files to check)` skips. The status badge uses yellow background (matching `Unimplemented` style) to distinguish from cyan `NoFiles` skips.

## CLI definition

**Flag:** `--report-level <LEVEL>`

Defined in `RunArgs` struct with:

- `value_enum` for clap parsing of the six level names
- `env = EnvVars::PREK_REPORT_LEVEL` for environment variable fallback
- `default_value_t = ReportLevel::Passed`

**Environment variable:** `PREK_REPORT_LEVEL` — added to `EnvVars` in `prek-consts`.

### Interaction with `--quiet`

The existing `-q`/`--quiet` flag controls the `Printer` level (`Quiet`, `Silent`) which suppresses all output broadly. `--report-level` is a finer-grained control that filters specifically which hook status lines appear. They compose independently: `-q` reduces all output at the printer level, while `--report-level` filters which hook statuses are emitted in the first place. No special interaction logic is needed — they operate at different layers.

## Files to modify

1. `**crates/prek-consts/src/env_vars.rs`\*\* — add `PREK_REPORT_LEVEL` constant
2. `**crates/prek/src/cli/mod.rs**` — add `report_level` field to `RunArgs`, define `ReportLevel` enum
3. `**crates/prek/src/cli/run/run.rs**` — main changes:
    - Add `RunStatus::Skipped` variant
    - Partition hooks into selected/skipped instead of filtering
    - Thread `report_level` through `run_hooks()` and `render_priority_group()`
    - Add `should_show()` gate before `status_printer.write()` calls
    - Render selector-skipped hooks after executed hooks per project
    - Suppress project headers and group UI when appropriate for the level
4. `**crates/prek/src/main.rs**` — pass `report_level` from args to `cli::run()`

## Testing

Tests are added to the existing `crates/prek/tests/skipped_hooks.rs`.

**Test cases:**

1. `--report-level fail` — mix of passing/failing hooks, only failed hooks in output
2. `--report-level silent` — no per-hook lines, exit code still reflects failures
3. `--report-level skipped-no-files` — failed + no-files hooks appear, passed hooks hidden
4. `--report-level skipped` — use `--skip` to exclude a hook, verify it appears with `(excluded by skip)` suffix
5. `--report-level passed` (default) — same output as current behavior
6. `--report-level all` — everything shows including selector-skipped hooks
7. `PREK_REPORT_LEVEL` env var — verify env var works as fallback
8. Default behavior — omitting the flag produces `passed`-level output

Existing test snapshots should not change since the default level matches current behavior.
