<div align="center">

<h1>
  <img width="180" alt="prek" src="./docs/assets/logo.webp" />
  <br/>prek
</h1>

[![prek](https://img.shields.io/endpoint?url=https://raw.githubusercontent.com/j178/prek/master/docs/assets/badge-v0.json)](https://github.com/j178/prek)
[![CI](https://github.com/j178/prek/actions/workflows/ci.yml/badge.svg)](https://github.com/j178/prek/actions/workflows/ci.yml)
[![codecov](https://codecov.io/github/j178/prek/graph/badge.svg?token=MP6TY24F43)](https://codecov.io/github/j178/prek)
[![GitHub Downloads](https://img.shields.io/github/downloads/j178/prek/total?logo=github)](https://github.com/j178/prek/releases)
[![PyPI Downloads](https://img.shields.io/pypi/dm/prek?logo=python)](https://pepy.tech/projects/prek)
[![Discord](https://img.shields.io/discord/1403581202102878289?logo=discord)](https://discord.gg/3NRJUqJz86)

</div>

<!-- description:start -->
[pre-commit](https://pre-commit.com/) is a framework to run hooks written in many languages, and it manages the
language toolchain and dependencies for running the hooks.

*prek* is a reimagined version of pre-commit, built in Rust.
It is designed to be a faster, dependency-free and drop-in alternative for it,
while also providing some additional long-requested features.
<!-- description:end -->

> [!NOTE]
> Although prek is pretty new, it’s already powering real‑world projects like [Apache Airflow](https://github.com/apache/airflow), and more projects are picking it up—see [Who is using prek?](#who-is-using-prek). If you’re looking for an alternative to `pre-commit`, please give it a try—we’d love your feedback!
>
> Please note that some subcommands and languages are still missing for full drop‑in parity with `pre-commit`. Track the remaining gaps here: [TODO](https://prek.j178.dev/todo/).

<!-- features:start -->
## Features

- 🚀 A single binary with no dependencies, does not require Python or any other runtime.
- ⚡ [Faster](https://prek.j178.dev/benchmark/) than `pre-commit` and more efficient in disk space usage.
- 🔄 Fully compatible with the original pre-commit configurations and hooks.
- 🏗️ Built-in support for monorepos (i.e. [workspace mode](https://prek.j178.dev/workspace/)).
- 🐍 Integration with [`uv`](https://github.com/astral-sh/uv) for managing Python virtual environments and dependencies.
- 🛠️ Improved toolchain installations for Python, Node.js, Go, Rust and Ruby, shared between hooks.
- 📦 [Built-in](https://prek.j178.dev/builtin/) Rust-native implementation of some common hooks.
<!-- features:end -->

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

<!-- linux-standalone-install:start -->
```bash
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/j178/prek/releases/download/v0.2.20/prek-installer.sh | sh
```
<!-- linux-standalone-install:end -->

On Windows:

<!-- windows-standalone-install:start -->
```powershell
powershell -ExecutionPolicy ByPass -c "irm https://github.com/j178/prek/releases/download/v0.2.20/prek-installer.ps1 | iex"
```
<!-- windows-standalone-install:end -->

</details>

<details>
<summary>PyPI</summary>

<!-- pypi-install:start -->
prek is published as Python binary wheel to PyPI, you can install it using `pip`, `uv` (recommended), or `pipx`:

```bash
# Using uv (recommended)
uv tool install prek

# Using uvx (install and run in one command)
uvx prek

# Using pip
pip install prek

# Using pipx
pipx install prek
```
<!-- pypi-install:end -->

</details>

<details>
<summary>Homebrew</summary>

<!-- homebrew-install:start -->
```bash
brew install prek
```
<!-- homebrew-install:end -->

</details>

<details>
<summary>mise</summary>

<!-- mise-install:start -->
To use prek with [mise](https://mise.jdx.dev) ([v2025.8.11](https://github.com/jdx/mise/releases/tag/v2025.8.11) or later):

```bash
mise use prek
```
<!-- mise-install:end -->

</details>

<details>
<summary>Cargo binstall</summary>

<!-- cargo-binstall:start -->
Install pre-compiled binaries from GitHub using [cargo-binstall](https://github.com/cargo-bins/cargo-binstall):

```bash
cargo binstall prek
```
<!-- cargo-binstall:end -->

</details>

<details>
<summary>Cargo</summary>

<!-- cargo-install:start -->
Build from source using Cargo (Rust 1.89+ is required):

```bash
cargo install --locked prek
```
<!-- cargo-install:end -->

</details>

<details>
<summary>npmjs</summary>

<!-- npmjs-install:start -->
prek is published as a Node.js package, you can install it using `npm`, `pnpm`, or `npx`:

```bash
# Using npm
npm add -D @j178/prek

# Using pnpm
pnpm add -D @j178/prek

# Using npx
npx @j178/prek --version

# or install globally
npm install -g @j178/prek

# then use `prek` command
prek --version
```
<!-- npmjs-install:end -->

</details>

<details>
<summary>Nix</summary>

<!-- nix-install:start -->
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
<!-- nix-install:end -->

</details>

<details>
<summary>Conda</summary>

<!-- conda-forge-install:start -->
prek is available as `prek` via [conda-forge](https://anaconda.org/conda-forge/prek).

```shell
conda install conda-forge::prek
```
<!-- conda-forge-install:end -->

</details>

<details>
<summary>Scoop (Windows)</summary>

<!-- scoop-install:start -->
prek is available via [Scoop](https://scoop.sh/#/apps?q=prek).

```powershell
scoop install main/prek
```
<!-- scoop-install:end -->
</details>

<details>
<summary>MacPorts</summary>

<!-- macports-install:start -->
prek is available via [MacPorts](https://ports.macports.org/port/prek/).

```bash
sudo port install prek
```
<!-- macports-install:end -->
</details>

<details>
<summary>GitHub Releases</summary>

<!-- pre-built-binaries:start -->
Pre-built binaries are available for download from the [GitHub releases](https://github.com/j178/prek/releases) page.
<!-- pre-built-binaries:end -->

</details>

<details>
<summary>GitHub Actions</summary>

<!-- github-actions:start -->
prek can be used in GitHub Actions via the [j178/prek-action](https://github.com/j178/prek-action) repository.

Example workflow:

```yaml
name: Prek checks
on: [push, pull_request]

jobs:
  prek:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v5
      - uses: j178/prek-action@v1
```

This action installs prek and runs `prek run --all-files` on your repository.
<!-- github-actions:end -->
</details>

<!-- self-update:start -->
If installed via the standalone installer, prek can update itself to the latest version:

```bash
prek self update
```
<!-- self-update:end -->

## Quick start

- **I already use pre-commit:** follow the short migration checklist in the [quickstart guide](https://prek.j178.dev/quickstart/#already-using-pre-commit) to swap in `prek` safely.
- **I'm new to pre-commit-style tools:** learn the basics—creating a config, running hooks, and installing git hooks—in the [beginner quickstart walkthrough](https://prek.j178.dev/quickstart/#new-to-pre-commit-style-workflows).

<!-- why:start -->
## Why prek?

### prek is faster

- It is [multiple times faster](https://prek.j178.dev/benchmark/) than `pre-commit` and takes up half the disk space.
- It redesigned how hook environments and toolchains are managed, they are all shared between hooks, which reduces the disk space usage and speeds up the installation process.
- Repositories are cloned in parallel, and hooks are installed in parallel if their dependencies are disjoint.
- It uses [`uv`](https://github.com/astral-sh/uv) for creating Python virtualenvs and installing dependencies, which is known for its speed and efficiency.
- It implements some common hooks in Rust, [built in prek](https://prek.j178.dev/builtin/), which are faster than their Python counterparts.
- It supports `repo: builtin` for offline, zero-setup hooks, which is not available in `pre-commit`.

### prek provides a better user experience

- No need to install Python or any other runtime, just download a single binary.
- No hassle with your Python version or virtual environments, prek automatically installs the required Python version and creates a virtual environment for you.
- Built-in support for [workspaces](https://prek.j178.dev/workspace/) (or monorepos), each subproject can have its own `.pre-commit-config.yaml` file.
- [`prek run`](https://prek.j178.dev/cli/#prek-run) has some nifty improvements over `pre-commit run`, such as:
  - `prek run --directory <dir>` runs hooks for files in the specified directory, no need to use `git ls-files -- <dir> | xargs pre-commit run --files` anymore.
  - `prek run --last-commit` runs hooks for files changed in the last commit.
  - `prek run [HOOK] [HOOK]` selects and runs multiple hooks.
- [`prek list`](https://prek.j178.dev/cli/#prek-list) command lists all available hooks, their ids, and descriptions, providing a better overview of the configured hooks.
- prek provides shell completions for `prek run <hook_id>` command, making it easier to run specific hooks without remembering their ids.

For more detailed improvements prek offers, take a look at [Difference from pre-commit](https://prek.j178.dev/diff/).

## Who is using prek?

prek is pretty new, but it is already being used or recommend by some projects and organizations:

- [Apache Airflow](https://github.com/apache/airflow/issues/44995)
- [PDM](https://github.com/pdm-project/pdm/pull/3593)
- [basedpyright](https://github.com/DetachHead/basedpyright/pull/1413)
- [OpenLineage](https://github.com/OpenLineage/OpenLineage/pull/3965)
- [Authlib](https://github.com/authlib/authlib/pull/804)
- [pre-commit-crocodile](https://radiandevcore.gitlab.io/tools/pre-commit-crocodile/)
- [PaperQA2](https://github.com/Future-House/paper-qa/pull/1098)
- [requests-cache](https://github.com/requests-cache/requests-cache/pull/1116)
- [kreuzberg](https://github.com/Goldziher/kreuzberg/pull/142)
- [attrs](https://github.com/python-attrs/attrs/commit/c95b177682e76a63478d29d040f9cb36a8d31915#diff-ef2cef9f88b4fe09ca3082140e67f5ad34fb65fb6e228f119d3812261ae51449)
- [FastMCP](https://github.com/jlowin/fastmcp/pull/2309)
- [iceberg-python](https://github.com/apache/iceberg-python/pull/2533)
- [msgspec](https://github.com/jcrist/msgspec/pull/918)
- [humanize](https://github.com/python-humanize/humanize/pull/276)
- [nf-core](https://github.com/nf-core/tools/pull/3899)

<!-- why:end -->

## Acknowledgements

This project is heavily inspired by the original [pre-commit](https://pre-commit.com/) tool, and it wouldn't be possible without the hard work
of the maintainers and contributors of that project.

And a special thanks to the [Astral](https://github.com/astral-sh) team for their remarkable projects, particularly [uv](https://github.com/astral-sh/uv),
from which I've learned a lot on how to write efficient and idiomatic Rust code.
