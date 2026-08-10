## Code Quality

- Write elegant, efficient, and concise code. Prefer simple, clear designs;
  concise does not mean clever.
- Prefer the smallest coherent change. Reuse existing mechanisms and avoid wrappers
  or abstractions for speculative or unmeasured gains.
- Prefer direct `if` or `match` control flow and explicit state types over clever
  combinators, wrappers, or invalid boolean/`Option` combinations. Prefer plain
  `if`/`else` to `.then()` or `.then_some()`.
- Try hard to avoid `panic!`, `unreachable!`, `.unwrap()`, and `.expect()`. Encode
  those constraints in the type system instead. More explicit code or a larger
  refactor is acceptable when it avoids these calls.
- Do not narrate code in comments. Explain invariants and unusual decisions in plain
  language that makes sense to a first-time reader. Be more verbose when clarity
  requires it.
- Fix Clippy findings. If suppression is necessary, prefer a narrow
  `#[expect(...)]` with a reason.

## Writing Style

- Write natural, direct English that sounds like a human collaborator, not a
  template or an AI-generated response.
- Avoid canned introductions, generic filler, repetition, and unnecessary
  summaries.
- Avoid dash-heavy prose, especially em dashes or `-` used to join thoughts.
  Prefer a period, comma, colon, or an explicit conjunction. For example, prefer
  `This is safe because the fallback still validates the state.` over
  `This is safe - the fallback still validates the state.`

## Build and Tests

- Linked worktrees must share the primary Cargo target. Before running `cargo` or a
  Cargo-backed `mise` task, set:

    ```sh
    export CARGO_TARGET_DIR="$(dirname "$(git rev-parse --path-format=absolute --git-common-dir)")/target"
    ```

- Check existing coverage and nearby style before adding a test. Prefer focused
  tests; do not run the full integration suite by default.

- In integration tests, prefer `cmd_snapshot!` snapshot coverage over
  string assertions such as `contains`.

- Regenerate `insta` snapshots with the relevant test and review them with
  `cargo insta review`; do not edit them manually.

## GitHub Interaction

- Draft GitHub comments locally. Do not post, submit reviews, resolve threads, or
  otherwise mutate GitHub state without explicit authorization.

## Pull Requests

- Keep commits focused with short imperative subjects. Never publish unrelated
  history or changes.
- Use the shortest practical branch name without a `codex/` prefix.
- Use a moderately descriptive title that clearly states the core action while
  remaining concise. Prefer `Build and reuse a run-scoped file index` over
  `Build run file index once`.
- Keep the PR body short; it may be only `Closes #<issue no>`.
- Do not include verification steps in the PR body.
- Add an appropriate `bug`, `enhancement`, `performance`, `internal`, `ci`, or
  `documentation` label.
- Do not create a draft PR unless explicitly requested.
- On an existing PR branch, amend only a trivial same-scope change; otherwise add a
  commit. Do not push or alter review state unless explicitly requested.
