<div align="center">

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="https://raw.githubusercontent.com/j178/prek/master/docs/assets/logo-dark.png">
  <img width="600" alt="prek" src="https://raw.githubusercontent.com/j178/prek/master/docs/assets/logo.png" />
</picture>

<a href="https://trendshift.io/repositories/14578?utm_source=repository-badge&amp;utm_medium=badge&amp;utm_campaign=badge-repository-14578" target="_blank" rel="noopener noreferrer"><img src="https://trendshift.io/api/badge/repositories/14578" alt="j178%2Fprek | Trendshift" width="250" height="55"/></a>

[![prek](https://img.shields.io/endpoint?url=https://raw.githubusercontent.com/j178/prek/master/docs/assets/badge-v0.json)](https://github.com/j178/prek)
[![PyPI version](https://img.shields.io/pypi/v/prek.svg)](https://pypi.python.org/pypi/prek)
[![codecov](https://codecov.io/github/j178/prek/graph/badge.svg?token=MP6TY24F43)](https://codecov.io/github/j178/prek)
[![PyPI Downloads](https://static.pepy.tech/personalized-badge/prek?period=monthly&units=INTERNATIONAL_SYSTEM&left_color=GREY&right_color=BLUE&left_text=downloads%2Fmonth)](https://pepy.tech/projects/prek)
[![Discord](https://img.shields.io/discord/1403581202102878289?logo=discord)](https://discord.gg/3NRJUqJz86)

**[Installation](#installation) • [Quick start](#quick-start) • [Documentation](https://prek.j178.dev/)**

</div>

## About

<!-- --8<-- [start: description] -->

prek is a framework for running hooks on your code. It runs them before you commit changes, on demand, or in CI.
These hooks can format files, catch lint errors, detect secrets, or run any other command your project defines.
prek also installs and manages the tools and dependencies they need.

You may already be familiar with the [pre-commit](https://pre-commit.com/) tool. prek is a reimagined version of it,
built in Rust. It is faster and distributed as a single binary with no runtime dependencies. It is fully compatible
with pre-commit configurations and hooks, so you can use it as a drop-in replacement without changing your setup.

<!-- --8<-- [end: description] -->

Although prek is pretty new, it’s already powering real‑world projects like [CPython](https://github.com/python/cpython), [Apache Airflow](https://github.com/apache/airflow), [FastAPI](https://github.com/fastapi/fastapi), and more projects are picking it up—see [Who is using prek?](#who-is-using-prek). If you’re looking for an alternative to `pre-commit`, please give it a try—we’d love your feedback!

<!-- --8<-- [start:features] -->

## Features

- A single binary with no dependencies, does not require Python or any other runtime.
- [Faster](https://prek.j178.dev/benchmark/) than `pre-commit` and more efficient in disk space usage.
- Fully compatible with the original pre-commit configurations and hooks.
- Built-in support for monorepos (i.e. [workspace mode](https://prek.j178.dev/workspace/)), including concurrent execution for independent same-depth projects.
- Integration with [`uv`](https://github.com/astral-sh/uv) for managing Python virtual environments and dependencies.
- Improved toolchain installations for Python, Node.js, Bun, Go, Rust and Ruby, shared between hooks.
- [Built-in](https://prek.j178.dev/builtin/) Rust-native implementation of some common hooks.

<!-- --8<-- [end:features] -->

## Installation

<details>
<summary>Standalone installer</summary>

prek provides a standalone installer script to download and install the tool,

On Linux and macOS:

<!-- --8<-- [start: linux-standalone-install] -->

```bash
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/j178/prek/releases/download/v0.5.1/prek-installer.sh | sh
```

<!-- --8<-- [end: linux-standalone-install] -->

On Windows:

<!-- --8<-- [start: windows-standalone-install] -->

```powershell
powershell -ExecutionPolicy ByPass -c "irm https://github.com/j178/prek/releases/download/v0.5.1/prek-installer.ps1 | iex"
```

<!-- --8<-- [end: windows-standalone-install] -->

</details>

<details>
<summary>PyPI</summary>

<!-- --8<-- [start: pypi-install] -->

prek is published as Python binary wheel to PyPI, you can install it using `pip`, `uv` (recommended), or `pipx`:

```bash
# Using uv (recommended)
uv tool install prek

# Using uvx (install and run in one command)
uvx prek

# Adding prek to the project dev-dependencies
uv add --dev prek

# Using pip
pip install prek

# Using pipx
pipx install prek
```

<!-- --8<-- [end: pypi-install] -->

</details>

<details>
<summary>Homebrew</summary>

<!-- --8<-- [start: homebrew-install] -->

```bash
brew install prek
```

<!-- --8<-- [end: homebrew-install] -->

</details>

<details>
<summary>mise</summary>

<!-- --8<-- [start: mise-install] -->

To use prek with [mise](https://mise.jdx.dev) ([v2025.8.11](https://github.com/jdx/mise/releases/tag/v2025.8.11) or later):

```bash
mise use prek
```

<!-- --8<-- [end: mise-install] -->

</details>

<details>
<summary>Cargo binstall</summary>

<!-- --8<-- [start: cargo-binstall] -->

Install pre-compiled binaries from GitHub using [cargo-binstall](https://github.com/cargo-bins/cargo-binstall):

```bash
cargo binstall prek
```

<!-- --8<-- [end: cargo-binstall] -->

</details>

<details>
<summary>Cargo</summary>

<!-- --8<-- [start: cargo-install] -->

Build from source using Cargo (Rust 1.96+ is required):

```bash
cargo install --locked prek
```

<!-- --8<-- [end: cargo-install] -->

</details>

<details>
<summary>npmjs</summary>

<!-- --8<-- [start: npmjs-install] -->

prek is published as a [Node.js package](https://www.npmjs.com/package/@j178/prek)
and can be installed with any npm-compatible package manager:

```bash
# As a dev dependency
npm add -D @j178/prek
pnpm add -D @j178/prek
bun add -D @j178/prek

# Or install globally
npm install -g @j178/prek
pnpm add -g @j178/prek
bun install -g @j178/prek

# Or run directly without installing
npx @j178/prek --version
bunx @j178/prek --version
```

<!-- --8<-- [end: npmjs-install] -->

</details>

<details>
<summary>Nix</summary>

<!-- --8<-- [start: nix-install] -->

prek is available via [Nixpkgs](https://search.nixos.org/packages?channel=unstable&show=prek&query=prek).

```shell
# Choose what's appropriate for your use case.
# One-off in a shell:
nix-shell -p prek

# NixOS or non-NixOS without flakes:
nix-env -iA nixos.prek

# Non-NixOS with flakes:
nix profile install nixpkgs#prek
```

<!-- --8<-- [end: nix-install] -->

</details>

<details>
<summary>Conda</summary>

<!-- --8<-- [start: conda-forge-install] -->

prek is available as `prek` via [conda-forge](https://anaconda.org/conda-forge/prek).

```shell
conda install conda-forge::prek
```

<!-- --8<-- [end: conda-forge-install] -->

</details>

<details>
<summary>Scoop (Windows)</summary>

<!-- --8<-- [start: scoop-install] -->

prek is available via [Scoop](https://scoop.sh/#/apps?q=prek).

```powershell
scoop install main/prek
```

<!-- --8<-- [end: scoop-install] -->

</details>

<details>
<summary>Winget (Windows)</summary>

<!-- --8<-- [start: winget-install] -->

prek is available via [winget](https://learn.microsoft.com/en-us/windows/package-manager/winget/).

```powershell
winget install --id j178.Prek
```

<!-- --8<-- [end: winget-install] -->

</details>

<details>
<summary>MacPorts</summary>

<!-- --8<-- [start: macports-install] -->

prek is available via [MacPorts](https://ports.macports.org/port/prek/).

```bash
sudo port install prek
```

<!-- --8<-- [end: macports-install] -->

</details>

<details>
<summary>GitHub Releases</summary>

<!-- --8<-- [start: pre-built-binaries] -->

Pre-built binaries are available for download from the [GitHub releases](https://github.com/j178/prek/releases) page.

<!-- --8<-- [end: pre-built-binaries] -->

</details>

<details>
<summary>GitHub Actions</summary>

<!-- --8<-- [start: github-actions] -->

prek can be used in GitHub Actions via the [j178/prek-action](https://github.com/j178/prek-action) repository.

Example workflow:

```yaml
name: Prek checks
on: [push, pull_request]

jobs:
  prek:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7
      - uses: j178/prek-action@4e14d07f9231acabce116ccfca13b13dd9755ece # v3.0.0
```

This action installs prek and runs `prek run --all-files` on your repository.

prek is also available via [`taiki-e/install-action`](https://github.com/taiki-e/install-action) for installing various tools.

<!-- --8<-- [end: github-actions] -->

</details>

<details>
<summary>prek skill for agents</summary>

<!-- --8<-- [start: gh-skill-install] -->

To let agents use `prek`, install the `prek` skill with `gh skill` (`v2.90.0+`):

```bash
gh skill install j178/prek prek
```

<!-- --8<-- [end: gh-skill-install] -->

</details>

<!-- --8<-- [start: self-update] -->

If installed via the standalone installer, prek can update itself to the latest version:

```bash
prek self update
```

<!-- --8<-- [end: self-update] -->

## Quick start

- **I already use pre-commit:** follow the short migration checklist in the [quickstart guide](https://prek.j178.dev/quickstart/#already-using-pre-commit) to swap in `prek` safely.
- **I'm new to pre-commit-style tools:** learn the basics—creating a config, running hooks, and installing Git shims—in the [beginner quickstart walkthrough](https://prek.j178.dev/quickstart/#new-to-pre-commit-style-workflows).

<!-- --8<-- [start: why] -->

## Why prek?

### prek is faster

- It is [multiple times faster](https://prek.j178.dev/benchmark/) than `pre-commit` while also using less disk space.
- Hook environments and toolchains are shared across hooks instead of being duplicated per repository, which reduces both install time and cache size.
- Repository fetches and independent hook environment setup run in parallel, hooks can run concurrently by [`priority`](https://prek.j178.dev/reference/configuration/#priority) using reusable [aliases](https://prek.j178.dev/reference/configuration/#priorities), and independent workspace projects at the same directory depth can run concurrently.
- It uses [`uv`](https://github.com/astral-sh/uv) for creating Python virtualenvs and installing dependencies, which is known for its speed and efficiency.
- For supported hooks from `pre-commit-hooks`, the [automatic fast path](https://prek.j178.dev/builtin/#1-automatic-fast-path) runs built-in Rust implementations without requiring any configuration changes.
- The prek-only `repo: builtin` mode provides offline, zero-setup hooks, including native `deny-pattern` and `require-pattern` alternatives for common `pygrep` checks.

### prek is easier to work with

- No need to install Python or any other runtime just to use `prek`; it is a single binary.
- Its [language support](https://prek.j178.dev/languages/) covers every language available in `pre-commit`, plus Bun, Deno, mise, and PHP, and it automatically installs managed toolchains when needed for Python, Node.js, Bun, Deno, Go, mise, Rust, and Ruby.
- It supports native [`prek.toml`](https://prek.j178.dev/configuration/) in addition to pre-commit YAML, and [`prek util yaml-to-toml`](https://prek.j178.dev/reference/cli/#prek-util-yaml-to-toml) helps migrate existing configs.
- Built-in support for [workspaces](https://prek.j178.dev/workspace/) means monorepos can keep separate configs per project and still run everything from one command, while independent same-depth projects run concurrently without mixing file scopes.
- [`prek install`](https://prek.j178.dev/reference/cli/#prek-install) and [`prek uninstall`](https://prek.j178.dev/reference/cli/#prek-uninstall) honor repo-local and worktree-local `core.hooksPath`.
- Hook [`groups`](https://prek.j178.dev/reference/configuration/#groups) let one config define workflows such as CI, linting, or formatting; `--group`, `--require-group`, and `--no-group` select them at runtime.
- [`prek run`](https://prek.j178.dev/reference/cli/#prek-run) can select or skip multiple projects and hooks, target tracked files with repeatable `--glob` or `--directory` filters, pass explicit paths with `--files`, and preview the selection with `--dry-run`.
- The progress UI streams a live preview from running hooks, so long-running checks do not look stuck and failures are easier to diagnose.
- [`prek list`](https://prek.j178.dev/reference/cli/#prek-list), [`prek util identify`](https://prek.j178.dev/reference/cli/#prek-util-identify), and [`prek util list-builtins -v`](https://prek.j178.dev/reference/cli/#prek-util-list-builtins) make it easier to inspect configured hooks, debug file matching, and discover builtins with their supported options.

### prek includes security-focused safeguards

- For supported managed toolchain downloads, `prek` verifies the downloaded archive or installer checksum before extracting or installing it, helping ensure the integrity of downloaded toolchains.
- [`prek update`](https://prek.j178.dev/reference/cli/#prek-update) can keep newly published releases on hold with `--cooldown-days`, filter eligible tags with glob patterns, and freeze revisions to commit SHAs.
- [`prek update`](https://prek.j178.dev/reference/cli/#prek-update) validates pinned SHA revisions against the fetched upstream refs, including impostor-commit detection, and keeps `# frozen:` comments in sync with the configured commit.
- [`prek update --check`](https://prek.j178.dev/reference/cli/#prek-update--check) is useful in CI when you want updates or frozen-reference mismatches to fail the job without rewriting the config.

For more detailed improvements prek offers, take a look at [Difference from pre-commit](https://prek.j178.dev/diff/).

## Who is using prek?

prek is pretty new, but it is already being used or recommended by some projects and organizations.
GitHub stars are current as of April 15, 2026.

- [apache/airflow](https://github.com/apache/airflow/issues/44995) <sub>45,050 stars</sub>
- [apache/iggy](https://github.com/apache/iggy/pull/2383) <sub>4,116 stars</sub>
- [apache/lucene](https://github.com/apache/lucene/pull/15629) <sub>3,401 stars</sub>
- [ast-grep/ast-grep](https://github.com/ast-grep/ast-grep.github.io/commit/e30818144b2967a7f9172c8cf2f4596bba219bf5) <sub>13,413 stars</sub>
- [astral-sh/ruff](https://github.com/astral-sh/ruff/pull/22505) <sub>47,070 stars</sub>
- [astral-sh/ty](https://github.com/astral-sh/ty/pull/2469) <sub>18,308 stars</sub>
- [authlib/authlib](https://github.com/authlib/authlib/pull/804) <sub>5,271 stars</sub>
- [cachix/devenv](https://github.com/cachix/devenv/pull/2304) <sub>6,665 stars</sub>
- [cocoindex-io/cocoindex](https://github.com/cocoindex-io/cocoindex/pull/1564) <sub>6,865 stars</sub>
- [commitizen-tools/commitizen](https://github.com/commitizen-tools/commitizen) <sub>3,377 stars</sub>
- [DetachHead/basedpyright](https://github.com/DetachHead/basedpyright/pull/1413) <sub>3,267 stars</sub>
- [django/djangoproject.com](https://github.com/django/djangoproject.com/pull/2252) <sub>1,994 stars</sub>
- [fastapi/asyncer](https://github.com/fastapi/asyncer/pull/437) <sub>2,407 stars</sub>
- [fastapi/fastapi](https://github.com/fastapi/fastapi/pull/14572) <sub>97,209 stars</sub>
- [fastapi/typer](https://github.com/fastapi/typer/pull/1453) <sub>19,210 stars</sub>
- [Future-House/paper-qa](https://github.com/Future-House/paper-qa/pull/1098) <sub>8,377 stars</sub>
- [getsentry/sentry](https://github.com/getsentry/sentry/pull/110808) <sub>43,639 stars</sub>
- [godotengine/godot](https://github.com/godotengine/godot/pull/119150) <sub>110,312 stars</sub>
- [home-assistant/core](https://github.com/home-assistant/core/pull/160427) <sub>86,029 stars</sub>
- [jcrist/msgspec](https://github.com/jcrist/msgspec/pull/918) <sub>3,692 stars</sub>
- [jlowin/fastmcp](https://github.com/jlowin/fastmcp/pull/2309) <sub>24,539 stars</sub>
- [MoonshotAI/kimi-cli](https://github.com/MoonshotAI/kimi-cli/pull/535) <sub>7,800 stars</sub>
- [openclaw/openclaw](https://github.com/openclaw/openclaw/pull/1720) <sub>357,512 stars</sub>
- [OpenLineage/OpenLineage](https://github.com/OpenLineage/OpenLineage/pull/3965) <sub>2,406 stars</sub>
- [pdm-project/pdm](https://github.com/pdm-project/pdm/pull/3593) <sub>8,553 stars</sub>
- [prowler-cloud/prowler](https://github.com/prowler-cloud/prowler/pull/10601) <sub>13,592 stars</sub>
- [pyodide/pyodide](https://github.com/pyodide/pyodide/pull/6182) <sub>14,527 stars</sub>
- [python-attrs/attrs](https://github.com/python-attrs/attrs/commit/c95b177682e76a63478d29d040f9cb36a8d31915) <sub>5,770 stars</sub>
- [python-telegram-bot/python-telegram-bot](https://github.com/python-telegram-bot/python-telegram-bot/pull/5142) <sub>29,025 stars</sub>
- [python/cpython](https://github.com/python/cpython/issues/143148) <sub>72,330 stars</sub>
- [simple-icons/simple-icons](https://github.com/simple-icons/simple-icons/pull/14245) <sub>24,873 stars</sub>

For a more comprehensive list of open-source projects using prek see the [list of dependents on github](https://github.com/j178/prek/network/dependents).

<!-- --8<-- [end: why] -->

## Acknowledgements

This project is heavily inspired by the original [pre-commit](https://pre-commit.com/) tool, and it wouldn't be possible without the hard work
of the maintainers and contributors of that project.

And a special thanks to the [Astral](https://github.com/astral-sh) team for their remarkable projects, particularly [uv](https://github.com/astral-sh/uv),
from which I've learned a lot on how to write efficient and idiomatic Rust code.
