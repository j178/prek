<div align="center">

<img width="600" alt="prek" src="https://raw.githubusercontent.com/j178/prek/master/docs/assets/logo.png" />

[![prek](https://img.shields.io/endpoint?url=https://raw.githubusercontent.com/j178/prek/master/docs/assets/badge-v0.json)](https://github.com/j178/prek)
[![PyPI version](https://img.shields.io/pypi/v/prek.svg)](https://pypi.python.org/pypi/prek)
[![codecov](https://codecov.io/github/j178/prek/graph/badge.svg?token=MP6TY24F43)](https://codecov.io/github/j178/prek)
[![PyPI Downloads](https://img.shields.io/pypi/dm/prek?logo=python)](https://pepy.tech/projects/prek)
[![Discord](https://img.shields.io/discord/1403581202102878289?logo=discord)](https://discord.gg/3NRJUqJz86)

</div>

<!-- --8<-- [start: description] -->

[pre-commit](https://pre-commit.com/) is a framework to run hooks written in many languages, and it manages the
language toolchain and dependencies for running the hooks.

*prek* is a reimagined version of pre-commit, built in Rust.
It is designed to be a faster, dependency-free and drop-in alternative for it,
while also providing some additional long-requested features.

<!-- --8<-- [end: description] -->

> [!NOTE]
> Although prek is pretty new, it’s already powering real‑world projects like [CPython](https://github.com/python/cpython), [Apache Airflow](https://github.com/apache/airflow), [FastAPI](https://github.com/fastapi/fastapi), and more projects are picking it up—see [Who is using prek?](#who-is-using-prek). If you’re looking for an alternative to `pre-commit`, please give it a try—we’d love your feedback!
>
> Please note that some languages are not yet supported for full drop‑in parity with `pre-commit`. See [Language Support](https://prek.j178.dev/languages/) for current status.

<!-- --8<-- [start:features] -->

## Features

- A single binary with no dependencies, does not require Python or any other runtime.
- [Faster](https://prek.j178.dev/benchmark/) than `pre-commit` and more efficient in disk space usage.
- Fully compatible with the original pre-commit configurations and hooks.
- Built-in support for monorepos (i.e. [workspace mode](https://prek.j178.dev/workspace/)).
- Integration with [`uv`](https://github.com/astral-sh/uv) for managing Python virtual environments and dependencies.
- Improved toolchain installations for Python, Node.js, Bun, Go, Rust and Ruby, shared between hooks.
- [Built-in](https://prek.j178.dev/builtin/) Rust-native implementation of some common hooks.

<!-- --8<-- [end:features] -->

## Table of contents

- [Installation](#installation)
- [Quick start](#quick-start)
- [Why prek?](#why-prek)
- [Who is using prek?](#who-is-using-prek)
- [Acknowledgements](#acknowledgements)

## Installation

<details>
<summary>Standalone installer</summary>

prek provides a standalone installer script to download and install the tool,

On Linux and macOS:

<!-- --8<-- [start: linux-standalone-install] -->

```bash
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/j178/prek/releases/download/v0.3.13/prek-installer.sh | sh
```

<!-- --8<-- [end: linux-standalone-install] -->

On Windows:

<!-- --8<-- [start: windows-standalone-install] -->

```powershell
powershell -ExecutionPolicy ByPass -c "irm https://github.com/j178/prek/releases/download/v0.3.13/prek-installer.ps1 | iex"
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

Build from source using Cargo (Rust 1.89+ is required):

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
      - uses: actions/checkout@v6
      - uses: j178/prek-action@v2
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
- Repositories are fetched in parallel, hook environments are prepared in parallel when their dependencies do not overlap, and hooks can run concurrently by [`priority`](https://prek.j178.dev/reference/configuration/#priority).
- It uses [`uv`](https://github.com/astral-sh/uv) for creating Python virtualenvs and installing dependencies, which is known for its speed and efficiency.
- It implements some common hooks in Rust as [builtins](https://prek.j178.dev/builtin/), which are faster than their Python counterparts.
- It supports `repo: builtin` for offline, zero-setup hooks, which is not available in `pre-commit`.

### prek is easier to work with

- No need to install Python or any other runtime just to use `prek`; it is a single binary.
- `prek` automatically installs the toolchains it needs for supported languages, so you spend less time managing Python versions, Node runtimes, Ruby installs, and similar setup.
- It supports native [`prek.toml`](https://prek.j178.dev/configuration/) in addition to pre-commit YAML, and [`prek util yaml-to-toml`](https://prek.j178.dev/reference/cli/#prek-util-yaml-to-toml) helps migrate existing configs.
- Built-in support for [workspaces](https://prek.j178.dev/workspace/) means monorepos can keep separate configs per project and still run everything from one command.
- [`prek install`](https://prek.j178.dev/reference/cli/#prek-install) and [`prek uninstall`](https://prek.j178.dev/reference/cli/#prek-uninstall) honor repo-local and worktree-local `core.hooksPath`.
- [`prek run`](https://prek.j178.dev/reference/cli/#prek-run) supports selecting or skipping multiple projects or hooks in workspace mode, instead of only accepting a single optional hook id, and adds quality-of-life improvements such as `--dry-run`, `--directory`, `--last-commit`, and `--no-fail-fast`.
- [`prek list`](https://prek.j178.dev/reference/cli/#prek-list), [`prek util identify`](https://prek.j178.dev/reference/cli/#prek-util-identify), and [`prek util list-builtins`](https://prek.j178.dev/reference/cli/#prek-util-list-builtins) make it easier to inspect configured hooks, debug file matching, and discover builtins.

### prek includes security-focused safeguards

- [`prek auto-update`](https://prek.j178.dev/reference/cli/#prek-auto-update) supports `--cooldown-days`, so you can keep newly published releases on hold for a cooling-off period before adopting them.
- [`prek auto-update`](https://prek.j178.dev/reference/cli/#prek-auto-update) validates pinned SHA revisions against the fetched upstream refs, including impostor-commit detection, and keeps `# frozen:` comments in sync with the configured commit.
- [`prek auto-update --check`](https://prek.j178.dev/reference/cli/#prek-auto-update--check) is useful in CI when you want updates or frozen-reference mismatches to fail the job without rewriting the config.

For more detailed improvements prek offers, take a look at [Difference from pre-commit](https://prek.j178.dev/diff/).

## Who is using prek?

prek is pretty new, but it is already being used or recommended by some projects and organizations.
GitHub stars are current as of April 15, 2026.

- [apache/airflow](https://github.com/apache/airflow/issues/44995) <sub>45,050 stars</sub>
- [python/cpython](https://github.com/python/cpython/issues/143148) <sub>72,330 stars</sub>
- [pdm-project/pdm](https://github.com/pdm-project/pdm/pull/3593) <sub>8,553 stars</sub>
- [fastapi/fastapi](https://github.com/fastapi/fastapi/pull/14572) <sub>97,209 stars</sub>
- [fastapi/typer](https://github.com/fastapi/typer/pull/1453) <sub>19,210 stars</sub>
- [fastapi/asyncer](https://github.com/fastapi/asyncer/pull/437) <sub>2,407 stars</sub>
- [astral-sh/ruff](https://github.com/astral-sh/ruff/pull/22505) <sub>47,070 stars</sub>
- [astral-sh/ty](https://github.com/astral-sh/ty/pull/2469) <sub>18,308 stars</sub>
- [openclaw/openclaw](https://github.com/openclaw/openclaw/pull/1720) <sub>357,512 stars</sub>
- [home-assistant/core](https://github.com/home-assistant/core/pull/160427) <sub>86,029 stars</sub>
- [python-telegram-bot/python-telegram-bot](https://github.com/python-telegram-bot/python-telegram-bot/pull/5142) <sub>29,025 stars</sub>
- [DetachHead/basedpyright](https://github.com/DetachHead/basedpyright/pull/1413) <sub>3,267 stars</sub>
- [OpenLineage/OpenLineage](https://github.com/OpenLineage/OpenLineage/pull/3965) <sub>2,406 stars</sub>
- [authlib/authlib](https://github.com/authlib/authlib/pull/804) <sub>5,271 stars</sub>
- [django/djangoproject.com](https://github.com/django/djangoproject.com/pull/2252) <sub>1,994 stars</sub>
- [Future-House/paper-qa](https://github.com/Future-House/paper-qa/pull/1098) <sub>8,377 stars</sub>
- [Goldziher/kreuzberg](https://github.com/Goldziher/kreuzberg/pull/142) <sub>7,550 stars</sub>
- [python-attrs/attrs](https://github.com/python-attrs/attrs/commit/c95b177682e76a63478d29d040f9cb36a8d31915) <sub>5,770 stars</sub>
- [jlowin/fastmcp](https://github.com/jlowin/fastmcp/pull/2309) <sub>24,539 stars</sub>
- [apache/iggy](https://github.com/apache/iggy/pull/2383) <sub>4,116 stars</sub>
- [apache/lucene](https://github.com/apache/lucene/pull/15629) <sub>3,401 stars</sub>
- [jcrist/msgspec](https://github.com/jcrist/msgspec/pull/918) <sub>3,692 stars</sub>
- [MoonshotAI/kimi-cli](https://github.com/MoonshotAI/kimi-cli/pull/535) <sub>7,800 stars</sub>
- [simple-icons/simple-icons](https://github.com/simple-icons/simple-icons/pull/14245) <sub>24,873 stars</sub>
- [ast-grep/ast-grep](https://github.com/ast-grep/ast-grep.github.io/commit/e30818144b2967a7f9172c8cf2f4596bba219bf5) <sub>13,413 stars</sub>
- [commitizen-tools/commitizen](https://github.com/commitizen-tools/commitizen) <sub>3,377 stars</sub>
- [cocoindex-io/cocoindex](https://github.com/cocoindex-io/cocoindex/pull/1564) <sub>6,865 stars</sub>
- [cachix/devenv](https://github.com/cachix/devenv/pull/2304) <sub>6,665 stars</sub>
- [pyodide/pyodide](https://github.com/pyodide/pyodide/pull/6182) <sub>14,527 stars</sub>
- [prowler-cloud/prowler](https://github.com/prowler-cloud/prowler/pull/10601) <sub>13,592 stars</sub>
- [getsentry/sentry](https://github.com/getsentry/sentry/pull/110808) <sub>43,639 stars</sub>
- [godotengine/godot](https://github.com/godotengine/godot/pull/119150) <sub>110,312 stars</sub>

<!-- --8<-- [end: why] -->

## Acknowledgements

This project is heavily inspired by the original [pre-commit](https://pre-commit.com/) tool, and it wouldn't be possible without the hard work
of the maintainers and contributors of that project.

And a special thanks to the [Astral](https://github.com/astral-sh) team for their remarkable projects, particularly [uv](https://github.com/astral-sh/uv),
from which I've learned a lot on how to write efficient and idiomatic Rust code.
