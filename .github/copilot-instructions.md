# Copilot instructions for `prek`

## Code requirements

- Concise, idiomatic Rust (2024 edition).
- Proper error handling (no unwraps, panics, etc. in app code).
- Clear separation of concerns (e.g., config parsing vs. execution).
- Thorough test coverage (unit + integration tests, snapshot testing where appropriate).

## Big picture

- Rust workspace under `crates/*`. The main CLI binary is `crates/prek` (`src/main.rs`).
- `prek` is a Rust reimplementation of `pre-commit`: configuration parsing lives in `crates/prek/src/config.rs`, execution/dispatch is in `crates/prek/src/run.rs`, and integration tests exercise the CLI end-to-end under `crates/prek/tests/`.
- User-facing output is centralized:
  - Warnings go through `warn_user!` / `warn_user_once!` in `crates/prek/src/warnings.rs` (can be disabled via `-q` / `-qq`).
  - Progress/output selection is via `Printer` in `crates/prek/src/printer.rs`.
- Cross-process coordination uses a store under `$PREK_HOME` (see `Store::from_settings` / `Store::lock_async` in `crates/prek/src/store.rs`).

## Developer workflows (preferred)

- Lint/format like CI: `mise run lint` (runs `cargo fmt` + `cargo clippy --all-targets --all-features --workspace -- -D warnings`).
- Run all tests: `mise run test` (workspace, all targets/features).
- Snapshot-first test workflow (insta):
  - Unit/bin tests with review UI: `mise run test-unit -- <filter>` or `mise run test-all-unit`.
  - Integration tests with review UI: `mise run test-integration <test> [filter]` or `mise run test-all-integration`.
  - Snapshots live under `crates/prek/src/snapshots/` and are used heavily by tests in `crates/prek/tests/`.

## Project-specific conventions

- Prefer `fs-err` / `fs-err::tokio` over `std::fs` / `tokio::fs` for filesystem operations (see many modules, e.g. `crates/prek/src/store.rs`).
- Prefer `anyhow::Result` for app-level flows and `thiserror` for typed errors when there’s a clear domain (see `crates/prek/src/store.rs`).
- Logging uses `tracing`; default behavior is configured in `crates/prek/src/main.rs` and can be overridden via `RUST_LOG`.
- CLI is defined with `clap` under `crates/prek/src/cli/` (entry in `crates/prek/src/cli/mod.rs`). If adding a command, keep wiring consistent with existing `cli/*` modules.

## Tests and fixtures

- Integration tests use `TestContext` helpers in `crates/prek/tests/common/mod.rs` and snapshot macros like `cmd_snapshot!`.
- Tests often normalize paths with regex filters; prefer using `context.filters()` for stable snapshots.
- DO NOT run whole-workspace tests in CI; they are slow. Use `cargo test -p prek --lib <unit-test> -- --exact` (or `cargo test -p prek --bin prek <unit-test> -- --exact`) for unit tests and `cargo test -p prek --test <test> -- <filter>` for integration tests.
- Use `cargo insta review --accept` to accept snapshot changes after running tests locally.

## Docs + generated artifacts

- Docs are built with MkDocs (see `mkdocs.yml`); run locally via `mise run build-docs`.
- CLI reference + JSON schema are generated via `mise run generate` (see tasks in `mise.toml`).

## When changing behavior

- If a change affects user-visible output, update the relevant snapshot(s) under `crates/prek/tests/` (use the `cargo insta` review flow).
- Keep output stable and routed through existing printer/warning macros rather than printing directly.
