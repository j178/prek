# Contributing to prek

Thanks for your interest in improving **prek**! This guide walks through the development environment, our snapshot-based testing workflow, and the helper tasks defined in `mise.toml` to keep everything smooth.

## 1. Set up the Rust development environment

1. **Install Rust with `rustup`** (recommended)

   Install `rustup` from <https://rustup.rs> if you do not already have it. Then install the toolchain pinned in `rust-toolchain.toml` (currently Rust 1.90):

   ```bash
   rustup show
   ```

   Finally, add the common developer components:

   ```bash
   rustup component add rustfmt clippy
   ```

1. **Install project helper tools**

   Install [`mise`](https://mise.jdx.dev/) to manage project-specific tools and tasks, then run `mise install` in the repository root to download the tool versions declared in `mise.toml` (for example `cargo-insta`, `cargo-nextest`, and the language toolchains used in integration tests).

1. (Optional) **Bootstrap git hooks**

   ```bash
   prek install
   ```

   This installs a `pre-push` git hook that keeps formatting and linting checks aligned with CI before you push changes.

## 2. Writing tests with `insta` snapshot assertions

prek uses [insta](https://insta.rs/) for snapshot testing. It's recommended (but not necessary) to use
`cargo-insta` for a better snapshot review experience.

If you are contributing new functionality, please include coverage via unit tests (in `src/…` using `#[cfg(test)]`) or integration tests (under `tests/`).

In integration tests, you can use `cmd_snapshot!` macro to simplify creating snapshots for prek commands. For example:

```rust
#[test]
fn test_run() {
    let context = TestContext::new();
    context.init_project();

    cmd_snapshot!(context.filters(), context.run(), @"");
}
```

## 3. Running tests and updating snapshots

You can invoke the test suite directly with Cargo or use the convenience tasks defined in `mise.toml`.

### Direct Cargo commands

- To run and review a specific snapshot test:

  ```bash
  cargo test --package <package> --test <test> -- <test_name> -- --exact
  cargo insta review
  ```

Where `<package>` is the crate name (for example, `prek`), `<test>` is the integration test file name (for example, `builtin_hooks`), and `<test_name>` is the specific test function name.

- Run snapshot-aware tests with the review UI:

  ```bash
  cargo insta test --review [test arguments]
  ```

  This command runs the selected tests, shows snapshot diffs, and lets you approve or reject updates interactively.

### Using mise tasks

`mise run <task>` picks up the arguments and environment declared in `mise.toml`. Helpful tasks include:

- `mise run test-unit -- <filter>` – run binary/unit tests matching `<filter>` with `cargo insta test --review --bin prek`.
- `mise run test-all-unit` – run all unit tests with snapshot review enabled.
- `mise run test-integration <test> [filter]` – run one integration test (for example `mise run test-integration builtin_hooks detect_private_key_hook`).
- `mise run test-all-integration` – execute the full integration test suite with review prompts.
- `mise run test` – run `cargo test` across the workspace without the snapshot review flow.
- `mise run lint` – run `cargo fmt` and `cargo clippy` (useful before opening a pull request).

## 4. Before you open a pull request

- Ensure `mise run lint` passes without errors.
- Include documentation updates if your change alters the user-facing behavior.
- Keep commits focused and write descriptive messages—this helps reviewers follow along.

Thanks again for contributing!
