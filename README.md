<div align="center">

# prek

<img width="220" alt="prek" src="https://github.com/user-attachments/assets/3a87815a-9f3c-48e7-8c1d-384b07ad3b35" />

[![CI](https://github.com/j178/prek/actions/workflows/ci.yml/badge.svg)](https://github.com/j178/prek/actions/workflows/ci.yml)
[![GitHub downloads](https://img.shields.io/github/downloads/j178/prek/total?logo=github)](https://github.com/j178/prek/releases)
[![PyPI Downloads](https://static.pepy.tech/badge/prek)](https://pepy.tech/projects/prek)
[![Discord](https://img.shields.io/discord/1403581202102878289?logo=discord)](https://discord.gg/3NRJUqJz86)

</div>

[pre-commit](https://pre-commit.com/) is a framework to run hooks written in many languages, and it manages the
language toolchain and dependencies for running the hooks.

**prek** is a reimagined version of pre-commit, built in Rust.
It is designed to be a faster, dependency-free and drop-in alternative for it,
while also providing some additional long-requested features.

> [!WARNING]
> prek is not production-ready yet, a few subcommands and languages are still in works. But it's already being adopted by [some projects](#who-are-using-prek), please give it a try - we'd love your feedback!
>
> Current supported languages are `python`, `node`, `go`, `docker`, `docker-image`, `pygrep`, `system`, `script` and `fail`.

## Features

- 🚀 A single binary with no dependencies, does not require Python or any other runtime.
- ⚡ About [10x faster](https://github.com/j178/prek/blob/master/docs/BENCHMARK.md) than `pre-commit` and uses only a third of disk space.
- 🔄 Fully compatible with the original pre-commit configurations and hooks.
- 🐍 Integration with [`uv`](https://github.com/astral-sh/uv) for managing Python virtual environments and dependencies.
- 🛠️ Improved toolchain installations for Python, Node.js, Go, Rust and Ruby, shared between hooks.
- 📦 Built-in implementation of some common hooks.
- 🏗️ (TODO) Built-in support for monorepos.

## How to migrate

prek is designed as a drop-in replacement:

- [Install prek](#installation)
- Replace `pre-commit` with `prek` in your commands
- Your existing `.pre-commit-config.yaml` works unchanged

```console
$ prek run
trim trailing whitespace.................................................Passed
fix end of files.........................................................Passed
typos....................................................................Passed
cargo fmt................................................................Passed
cargo clippy.............................................................Passed
```

For configuring `.pre-commit-config.yaml` and writing hooks, you can refer to the [pre-commit documentation](https://pre-commit.com/) as prek is fully compatible with it.

## Why prek?

### prek is way faster

- It is about [10x faster](https://github.com/j178/prek/blob/master/docs/BENCHMARK.md) than `pre-commit` and uses only a third of disk space.
- It redesigned how hook environments and toolchains are managed, they are all shared between hooks, which reduces the disk space usage and speeds up the installation process.
- Repositories are cloned in parallel, and hooks are installed in parallel if their dependencies are disjoint.
- It uses [`uv`](https://github.com/astral-sh/uv) for creating Python virtualenvs and installing dependencies, which is known for its speed and efficiency.
- It implements some common hooks in Rust, built in prek, which are faster than their Python counterparts.

### prek provides a better user experience

- No need to install Python or any other runtime, just download a single binary.
- No hassle with your Python version or virtual environments, prek automatically installs the required Python version and creates a virtual environment for you.
- (TODO): Built-in support for workspaces (or monorepos), each subproject can have its own `.pre-commit-config.yaml` file.
- `prek run` has some improvements over `pre-commit run`, such as:
    - `prek run --directory <dir>` runs hooks for files in the specified directory, no need to use `git ls-files -- <dir> | xargs pre-commit run --files` anymore.
    - `prek run --last-commit` runs hooks for files changed in the last commit.
- `prek list` command lists all available hooks, their ids, and descriptions, providing a better overview of the configured hooks.
- prek provides shell completions for `prek run <hook_id>` command, making it easier to run specific hooks without remembering their ids.

## Who are using prek?

prek is pretty new, but it is already being used or recommend by some projects and organizations:

- [Airflow](https://github.com/apache/airflow/issues/44995)
- [PDM](https://github.com/pdm-project/pdm/pull/3593)
- [basedpyright](https://github.com/DetachHead/basedpyright/pull/1413)
- [OpenLineage](https://github.com/OpenLineage/OpenLineage/pull/3965)
- [Authlib](https://github.com/authlib/authlib/pull/804)

## Installation

<details>
<summary>Standalone installer</summary>

prek provides a standalone installer script to download and install the tool:

```console
# On Linux and macOS
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/j178/prek/releases/download/v0.1.5/prek-installer.sh | sh

# On Windows
powershell -ExecutionPolicy ByPass -c "irm https://github.com/j178/prek/releases/download/v0.1.5/prek-installer.ps1 | iex"
```
</details>

<details>
<summary>PyPI</summary>

prek is published as Python binary wheel to PyPI, you can install it using `pip`, `uv` (recommended), or `pipx`:

```console
pip install prek

# or

uv tool install prek

# or

pipx install prek
```
</details>

<details>
<summary>Homebrew</summary>

```console
brew install prek
```
</details>

<details>
<summary>mise</summary>

```console
mise use prek
```
</details>

<details>
<summary>Cargo</summary>

Build from source using Cargo:

```console
cargo install --locked --git https://github.com/j178/prek
```
</details>

<details>
<summary>GitHub Releases</summary>

prek release artifacts can be downloaded directly from the [GitHub releases](https://github.com/j178/prek/releases).
</details>

If installed via the standalone installer, prek can update itself to the latest version:

```console
$ prek self update
```

## Acknowledgements

This project is heavily inspired by the original [pre-commit](https://pre-commit.com/) tool, and it wouldn't be possible without the hard work
of the maintainers and contributors of that project.

And a special thanks to the [Astral](https://github.com/astral-sh) team for their remarkable projects, particularly [uv](https://github.com/astral-sh/uv),
from which I've learned a lot on how to write efficient and idiomatic Rust code.
