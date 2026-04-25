---
name: prek
description: Use when setting up or running hooks with prek in any repository. prek is a Rust drop-in alternative to pre-commit for running Git hooks that check, format, lint, and validate code and repository files. Prefer it when speed, workspace mode, built-in hooks, shared toolchains, or native TOML configuration matter.
---

# prek

Use this skill when the user wants to set up or run Git hooks with `prek`.

`prek` is a Rust reimplementation of `pre-commit`. Its main job is to run hooks that check code and repository files before commit or on demand: formatters, linters, validators, security checks, and custom project checks.

## Start with the public docs

Use the LLM-oriented index first:

- [llms.txt](https://prek.j178.dev/llms.txt)

Per that file, prefer explicit markdown docs when you need details:

- [Introduction](https://prek.j178.dev/index.md)
- [Installation](https://prek.j178.dev/installation/index.md)
- [Quickstart](https://prek.j178.dev/quickstart/index.md)
- [Common Workflows](https://prek.j178.dev/usage/index.md)
- [Configuration](https://prek.j178.dev/configuration/index.md)
- [Workspace Mode](https://prek.j178.dev/workspace/index.md)
- [Language Support](https://prek.j178.dev/languages/index.md)
- [Built-in Hooks](https://prek.j178.dev/builtin/index.md)
- [CLI Reference](https://prek.j178.dev/reference/cli/index.md)
- [Configuration Reference](https://prek.j178.dev/reference/configuration/index.md)
- [Environment Variable Reference](https://prek.j178.dev/reference/environment-variables/index.md)
- [Differences from pre-commit](https://prek.j178.dev/diff/index.md)
- [Benchmark](https://prek.j178.dev/benchmark/index.md)

## What prek is for

Use `prek` to:

- run hooks that check source code and repo contents
- install Git hook shims such as `pre-commit`, `pre-push`, and `commit-msg`
- run the same hook ecosystem used by `pre-commit`
- manage hook runtimes and toolchains for supported languages
- validate configs, update pinned hook revisions, and inspect configured hooks

Typical jobs for hooks run by `prek`:

- formatting and linting code
- validating YAML, JSON, TOML, XML, and similar files
- preventing merge-conflict markers, private keys, oversized files, and bad line endings

## Authoring configs

For new configs, prefer `prek.toml`.

Important repo types:

- remote repo: normal hook repository such as `https://github.com/astral-sh/ruff-pre-commit`
- `repo = "local"`: hooks defined in the current repository
- `repo = "meta"`: config-checking hooks like `check-hooks-apply`, `check-useless-excludes`, and `identity`
- `repo = "builtin"`: `prek`'s offline Rust-native hooks

Minimal examples:

```toml
[[repos]]
repo = "https://github.com/astral-sh/ruff-pre-commit"
rev = "v0.14.3"
hooks = [
  { id = "ruff" },
  { id = "ruff-format" },
]
```

```toml
[[repos]]
repo = "local"
hooks = [
  {
    id = "cargo-fmt",
    name = "cargo fmt",
    language = "system",
    entry = "cargo fmt --",
    files = "\\.rs$",
  },
]
```

```toml
[[repos]]
repo = "builtin"
hooks = [
  { id = "trailing-whitespace" },
  { id = "check-yaml" },
]
```

These examples use TOML 1.1 multiline inline tables. Use `[[repos.hooks]]` array-of-tables if an editor or parser in the toolchain does not support that syntax yet, or when a hook has many fields such as `env`, `pass_filenames = false`, or `priority`.

Filtering patterns:

- regex is the most portable choice: `files = "\\.rs$"`
- `prek` also supports globs: `files = { glob = "src/**/*.rs" }`
- use glob lists for multiple roots: `exclude = { glob = ["target/**", "dist/**"] }`

Scheduling:

- smaller `priority` values run earlier
- hooks with the same `priority` can run concurrently
- `priority` is evaluated within one config file, not across workspace projects

Useful `prek`-specific hook/config fields when editing TOML:

- `env` for per-hook environment variables
- `priority` for hook ordering and concurrency
- `minimum_prek_version` for gating newer config features
- `orphan = true` to isolate a nested workspace project from parent configs

## Default workflow

When adopting `prek` in a repository:

1. Check whether the repo already has `.pre-commit-config.yaml` or `.pre-commit-config.yml`.
2. If it does, usually keep that config and switch the commands from `pre-commit` to `prek`.
3. If migrating from `pre-commit`, reinstall the Git shims with `prek install -f`.
4. If it does not, prefer creating `prek.toml` for a fresh setup.
5. Install `prek`.
6. Validate the config with `prek validate-config`.
7. Install the Git shims and prepare hook environments with `prek install --prepare-hooks`.
8. Run everything once with `prek run --all-files`.
9. For monorepos, consider nested configs, `.prekignore`, and `orphan: true`.

If the user explicitly wants maximum upstream portability, stay with `.pre-commit-config.yaml` and avoid `prek`-only keys.

## Install and run

Common install methods:

- `uv tool install prek`
- `brew install prek`
- `mise use prek`
- `cargo binstall prek`
- `cargo install --locked prek`

### Command guide

- `prek install`: install Git hook shims into the repo's effective hooks directory
- `prek prepare-hooks`: prepare hook environments without installing Git shims
- `prek install --prepare-hooks`: install shims and prepare environments in one step
- `prek run`: run hooks for the current staged file selection
- `prek run --all-files`: run hooks across the whole repository
- `prek run <hook-id>`: run only one hook
- `prek list`: list discovered hooks and projects
- `prek validate-config`: validate `prek.toml` or `.pre-commit-config.yaml`
- `prek auto-update`: update pinned hook revisions
- `prek util yaml-to-toml`: convert an existing YAML config to `prek.toml`
- `prek util identify <path>`: inspect file tags when `types`, `types_or`, or `exclude_types` do not match as expected

Useful quality-of-life commands and options mentioned in the docs:

- `prek run --dry-run`
- `prek run --directory <dir>`
- `prek run --last-commit`
- `prek run --skip <hook-or-project>`
- `prek -C <dir> ...`

For debugging:

- `prek run -vvv`
- `PREK_NO_FAST_PATH=1 prek run`: compare builtin fast-path behavior against the standard execution path
- Check the [Environment Variable Reference](https://prek.j178.dev/reference/environment-variables/index.md) for `PREK_*` controls such as `PREK_HOME`, `PREK_SKIP`, and concurrency limits.

## Built-in hook guidance

`prek` has two important builtin paths:

- automatic fast path for supported hooks from `https://github.com/pre-commit/pre-commit-hooks`
- explicit `repo: builtin` for offline, zero-setup built-in hooks

Reach for `repo: builtin` when speed, no-network setup, or minimal bootstrapping matters more than upstream `pre-commit` compatibility.

Builtin hooks called out by the docs include:

- `trailing-whitespace`
- `check-added-large-files`
- `check-case-conflict`
- `check-illegal-windows-names`
- `end-of-file-fixer`
- `file-contents-sorter`
- `fix-byte-order-marker`
- `check-json`
- `check-json5`
- `pretty-format-json`
- `check-toml`
- `check-vcs-permalinks`
- `check-yaml`
- `check-xml`
- `mixed-line-ending`
- `check-symlinks`
- `destroyed-symlinks`
- `check-merge-conflict`
- `detect-private-key`
- `no-commit-to-branch`
- `check-shebang-scripts-are-executable`
- `check-executables-have-shebangs`

## Practical guidance for agents

- Prefer `prek.toml` for new setups.
- Prefer existing `.pre-commit-config.yaml` for migrations unless the user asks for TOML.
- Reach for workspace mode in monorepos instead of forcing one giant root-only config.
- Consider `repo: builtin` when offline or zero-setup hooks are useful.
- If the repo already uses `pre-commit-hooks`, remember that `prek` can use built-in Rust implementations for some common hooks.
- Start with a small default hook set, then add language-specific hooks the project already uses.
- Use `prek util yaml-to-toml` instead of hand-converting YAML when migrating.
- Before promising parity for a specific hook language, verify it in [Language Support](https://prek.j178.dev/languages/index.md).
