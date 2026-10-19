# prek

prek is a framework for running hooks on your code. It runs them before you commit changes, on demand, or in CI. These hooks can format files, catch lint errors, detect secrets, or run any other command your project defines. prek also installs and manages the tools and dependencies they need.

You may already be familiar with the [pre-commit](<https://pre-commit.com/>) tool. prek is a reimagined version of it, built in Rust. It is faster and distributed as a single binary with no runtime dependencies. It is fully compatible with pre-commit configurations and hooks, so you can use it as a drop-in replacement without changing your setup.

> [!NOTE]
>
> Although prek is pretty new, it's already powering real‑world projects like [CPython](<https://github.com/python/cpython>), [Apache Airflow](<https://github.com/apache/airflow>), [FastAPI](<https://github.com/fastapi/fastapi>), and more projects are picking it up—see [Who is using prek?](<#who-is-using-prek>). If you're looking for an alternative to `pre-commit`, please give it a try—we'd love your feedback!

## Features

- A single binary with no dependencies, does not require Python or any other runtime.
- [Faster](<https://prek.j178.dev/benchmark/>) than `pre-commit` and more efficient in disk space usage.
- Fully compatible with the original pre-commit configurations and hooks.
- Built-in support for monorepos (i.e. [workspace mode](<https://prek.j178.dev/monorepos/>)), including concurrent execution for independent same-depth projects.
- Integration with [`uv`](<https://github.com/astral-sh/uv>) for managing Python virtual environments and dependencies.
- Improved toolchain installations for Python, Node.js, Bun, Go, Rust and Ruby, shared between hooks.
- [Built-in](<https://prek.j178.dev/built-in-hooks/>) Rust-native implementation of some common hooks.

## Where to Start

- New to `prek`: start with [Installation](<https://prek.j178.dev/dev/installation/index.md>), then follow the [Quickstart](<https://prek.j178.dev/dev/quickstart/index.md>).
- Already set up: use [Running Hooks](<https://prek.j178.dev/dev/running-hooks/index.md>) for day-to-day commands and handling hook failures.
- Replacing another hook tool: follow [Migrating from Other Hook Tools](<https://prek.j178.dev/dev/migration/index.md>).
- Writing config: read [Configuration](<https://prek.j178.dev/dev/configuration/index.md>), then use the [Configuration Reference](<https://prek.j178.dev/dev/reference/configuration/index.md>) for exact keys.
- Defining hooks directly in your project configuration: use [Local Hooks](<https://prek.j178.dev/dev/local-hooks/index.md>).
- Adding repository checks to a pipeline: see [Continuous Integration](<https://prek.j178.dev/dev/ci/index.md>).
- Looking for setup patterns: browse the [Cookbook](<https://prek.j178.dev/dev/cookbook/index.md>).
- Working in a monorepo: see [Monorepos](<https://prek.j178.dev/dev/monorepos/index.md>).
- Looking for flags or environment variables: use the [CLI Reference](<https://prek.j178.dev/dev/reference/cli/index.md>) and [Environment Variable Reference](<https://prek.j178.dev/dev/reference/environment-variables/index.md>).
- Reviewing trust and pinning choices: read the [Security Guide](<https://prek.j178.dev/dev/security/index.md>).

## Why prek?

### prek is faster

- It is [multiple times faster](<https://prek.j178.dev/benchmark/>) than `pre-commit` while also using less disk space.
- Hook environments and toolchains are shared across hooks instead of being duplicated per repository, which reduces both install time and cache size.
- Repository fetches and independent hook environment setup run in parallel, hooks can run concurrently by [`priority`](<https://prek.j178.dev/reference/configuration/#priority>) using reusable [aliases](<https://prek.j178.dev/reference/configuration/#priorities>), and independent workspace projects at the same directory depth can run concurrently.
- It uses [`uv`](<https://github.com/astral-sh/uv>) for creating Python virtualenvs and installing dependencies, which is known for its speed and efficiency.
- For supported hooks from `pre-commit-hooks`, the [automatic fast path](<https://prek.j178.dev/built-in-hooks/#use-the-automatic-fast-path>) runs built-in Rust implementations without requiring any configuration changes.
- The prek-only `repo: builtin` mode provides offline, zero-setup hooks, including native `deny-pattern` and `require-pattern` alternatives for common `pygrep` checks.

### prek is easier to work with

- No need to install Python or any other runtime just to use `prek`; it is a single binary.
- Its [language support](<https://prek.j178.dev/reference/language-support/>) covers every language available in `pre-commit`, plus Bun, Deno, mise, and PHP, and it automatically installs managed toolchains when needed for Python, Node.js, Bun, Deno, Go, mise, Rust, and Ruby.
- It supports native [`prek.toml`](<https://prek.j178.dev/configuration/>) in addition to pre-commit YAML, and [`prek util yaml-to-toml`](<https://prek.j178.dev/reference/cli/#prek-util-yaml-to-toml>) helps migrate existing configs.
- Built-in support for [workspaces](<https://prek.j178.dev/monorepos/>) means monorepos can keep separate configs per project and still run everything from one command, while independent same-depth projects run concurrently without mixing file scopes.
- [`prek install`](<https://prek.j178.dev/reference/cli/#prek-install>) and [`prek uninstall`](<https://prek.j178.dev/reference/cli/#prek-uninstall>) honor repo-local and worktree-local `core.hooksPath`.
- Hook [`groups`](<https://prek.j178.dev/reference/configuration/#groups>) let one config define workflows such as CI, linting, or formatting; `--group`, `--require-group`, and `--no-group` select them at runtime.
- [`prek run`](<https://prek.j178.dev/reference/cli/#prek-run>) can select or skip multiple projects and hooks, target tracked files with repeatable `--glob` or `--directory` filters, pass explicit paths with `--files`, and preview the selection with `--dry-run`.
- The progress UI streams a live preview from running hooks, so long-running checks do not look stuck and failures are easier to diagnose.
- [`prek list`](<https://prek.j178.dev/reference/cli/#prek-list>), [`prek util identify`](<https://prek.j178.dev/reference/cli/#prek-util-identify>), and [`prek util list-builtins -v`](<https://prek.j178.dev/reference/cli/#prek-util-list-builtins>) make it easier to inspect configured hooks, debug file matching, and discover builtins with their supported options.

### prek includes security-focused safeguards

- For supported managed toolchain downloads, `prek` verifies the downloaded archive or installer checksum before extracting or installing it, helping ensure the integrity of downloaded toolchains.
- [`prek update`](<https://prek.j178.dev/reference/cli/#prek-update>) can keep newly published releases on hold with `--cooldown-days`, filter eligible tags with glob patterns, and freeze revisions to commit SHAs.
- [`prek update`](<https://prek.j178.dev/reference/cli/#prek-update>) validates pinned SHA revisions against the fetched upstream refs, including impostor-commit detection, and keeps `# frozen:` comments in sync with the configured commit.
- [`prek update --check`](<https://prek.j178.dev/reference/cli/#prek-update--check>) is useful in CI when you want updates or frozen-reference mismatches to fail the job without rewriting the config.

For more detailed improvements prek offers, take a look at [Difference from pre-commit](<https://prek.j178.dev/diff/>).

## Who is using prek?

These projects use or recommend prek. Follow the links for adoption details.

| Project | Stars |
| --- | ---: |
| [Apache Airflow](<https://github.com/apache/airflow/pull/54258>) | 47,116 |
| [Apache Iggy](<https://github.com/apache/iggy/pull/2383>) | 5,044 |
| [ast-grep](<https://github.com/ast-grep/ast-grep.github.io/commit/e30818144b2967a7f9172c8cf2f4596bba219bf5>) | 16,140 |
| [attrs](<https://github.com/python-attrs/attrs/commit/c95b177682e76a63478d29d040f9cb36a8d31915>) | 5,853 |
| [Authlib](<https://github.com/authlib/authlib/pull/804>) | 5,430 |
| [Cherry Studio](<https://github.com/CherryHQ/cherry-studio/pull/12491>) | 52,443 |
| [CocoIndex](<https://github.com/cocoindex-io/cocoindex/pull/1564>) | 11,651 |
| [CPython](<https://github.com/python/cpython/pull/143149>) | 77,561 |
| [devenv](<https://github.com/cachix/devenv/pull/2304>) | 7,721 |
| [FastAPI](<https://github.com/fastapi/fastapi/pull/14572>) | 102,873 |
| [FastMCP](<https://github.com/PrefectHQ/fastmcp/pull/2309>) | 28,011 |
| [Godot](<https://github.com/godotengine/godot/pull/119150>) | 118,258 |
| [Home Assistant](<https://github.com/home-assistant/core/pull/160427>) | 91,313 |
| [Kimi CLI](<https://github.com/MoonshotAI/kimi-cli/pull/535>) (archived) | 11,425 |
| [Matplotlib](<https://github.com/matplotlib/matplotlib/pull/31081>) | 23,341 |
| [msgspec](<https://github.com/msgspec/msgspec/pull/918>) | 4,155 |
| [OpenClaw](<https://github.com/openclaw/openclaw/pull/1720>) | 391,637 |
| [paper-qa](<https://github.com/Future-House/paper-qa/pull/1098>) | 9,329 |
| [PDM](<https://github.com/pdm-project/pdm/blob/main/CONTRIBUTING.md>) | 8,664 |
| [Pendulum](<https://github.com/python-pendulum/pendulum/pull/946>) | 6,677 |
| [Prowler](<https://github.com/prowler-cloud/prowler/pull/10601>) | 14,985 |
| [Pyodide](<https://github.com/pyodide/pyodide/pull/6182>) | 14,880 |
| [python-telegram-bot](<https://github.com/python-telegram-bot/python-telegram-bot/pull/5142>) | 29,515 |
| [RSSHub](<https://github.com/DIYgod/RSSHub/blob/master/package.json>) | 46,449 |
| [Ruff](<https://github.com/astral-sh/ruff/pull/22505>) | 49,936 |
| [Sentry](<https://github.com/getsentry/sentry/pull/110808>) | 45,507 |
| [Simple Icons](<https://github.com/simple-icons/simple-icons/pull/14245>) | 25,985 |
| [ty](<https://github.com/astral-sh/ty/pull/2469>) | 19,817 |
| [Typer](<https://github.com/fastapi/typer/pull/1453>) | 20,056 |
| [UltraJSON](<https://github.com/ultrajson/ultrajson/pull/744>) | 4,499 |

GitHub stars as of October 8, 2026. For more projects, see the [GitHub dependents list](<https://github.com/j178/prek/network/dependents>).

## Badges

Show that your project uses prek with a badge in your README:

```markdown
[![prek](https://raw.githubusercontent.com/j178/prek/master/docs/assets/badge.svg)](https://github.com/j178/prek)
```

```html
<a href="https://github.com/j178/prek">
  <img src="https://raw.githubusercontent.com/j178/prek/master/docs/assets/badge.svg" alt="prek">
</a>
```

```rst
.. image:: https://raw.githubusercontent.com/j178/prek/master/docs/assets/badge.svg
   :target: https://github.com/j178/prek
   :alt: prek
```
