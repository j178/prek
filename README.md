# prefligit

![Development Status](https://img.shields.io/badge/Development-Early_Stage-yellowgreen)
[![CI](https://github.com/j178/prefligit/actions/workflows/ci.yml/badge.svg)](https://github.com/j178/prefligit/actions/workflows/ci.yml)
[![GitHub downloads](https://img.shields.io/github/downloads/j178/prefligit/total)](https://github.com/j178/prefligit/releases)

<img width="250" alt="prefligit" src="https://github.com/user-attachments/assets/49080cb0-f528-4aa5-acb7-5a88eb9eff4a">

[pre-commit](https://pre-commit.com/) is a framework to run hooks written in many languages, and it manages the language toolchain and dependencies for running the hooks.

prefligit is a reimagined version of pre-commit, built in Rust. It is designed to be a faster, dependency-free and drop-in alternative for it, while also providing some additional long-requested features.

> [!WARNING]
> This project is still in early stage of development, some features are still not implemented.
> It is not production-ready yet, but feel free to try it out and provide feedback.
>
> Current supported languages are `python`, `node`, `go`, `docker`, `docker-image`, `system`, `script` and `fail`.

## Features

- 🚀 A single binary with no dependencies, does not require Python or any other runtime.
- ⚡ About [10x faster](https://github.com/j178/prefligit/blob/master/BENCHMARK.md) than `pre-commit` and uses only a third of disk space.
- 🔄 Fully compatible with the original pre-commit configurations and hooks.
- 🐍 Integration with [`uv`](https://github.com/astral-sh/uv) for managing Python virtual environments and dependencies.
- 🛠️ Improved toolchain installations for Python, Node.js, Go, Rust and Ruby, shared between hooks.
- 📦 Built-in implementation of some common hooks.
- 🏗️ (TODO) Built-in support for monorepos.

## How to migrate

prefligit is designed as a drop-in replacement:

- [Install prefligit](#installation).
- Replace `pre-commit` with `prefligit` in your commands
- Your existing `.pre-commit-config.yaml` works unchanged

```console
$ prefligit run
trim trailing whitespace.................................................Passed
fix end of files.........................................................Passed
typos....................................................................Passed
cargo fmt................................................................Passed
cargo clippy.............................................................Passed
```

For configuring `.pre-commit-config.yaml` and writing hooks, you can refer to the [pre-commit documentation](https://pre-commit.com/) as prefligit is fully compatible with it.

## Why prefligit?

### prefligit is way faster

- It is about [10x faster](https://github.com/j178/prefligit/blob/master/BENCHMARK.md) than `pre-commit` and uses only a third of disk space.
- It redesigned how hook environments and toolchains are managed, they are all shared between hooks, which reduces the disk space usage and speeds up the installation process.
- Repositories are cloned in parallel, and hooks are installed in parallel if their dependencies are disjoint.
- It uses [`uv`](https://github.com/astral-sh/uv) for creating Python virtualenvs and installing dependencies, which is known for its speed and efficiency.
- It implements some common hooks in Rust, built in prefligit, which are faster than their Python counterparts.

### prefligit provides a better user experience

- No need to install Python or any other runtime, just download a single binary.
- No hassle with your Python version or virtual environments, prefligit automatically installs the required Python version and creates a virtual environment for you.
- (TODO): Built-in support for workspaces (or monorepos), each sub-project can have its own `.pre-commit-config.yaml` file.
- `prefligit run` has some improvements over `pre-commit run`, such as:
    - `prefligit run --directory <dir>` runs hooks for files in the specified directory, no need to use `git ls-files -- <dir> | xargs pre-commit run --files` anymore.
    - `prefligit run --last-commit` runs hooks for files changed in the last commit.
- (TODO): prefligit provides shell completions for `prefligit run <hook_id>` command, so you can easily find the available hooks.

## Installation

<details>
<summary>Standalone installer</summary>

prefligit provides a standalone installer script to download and install the tool:

```console
# On Linux and macOS
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/j178/prefligit/releases/download/v0.0.22/prefligit-installer.sh | sh

# On Windows
powershell -ExecutionPolicy ByPass -c "irm https://github.com/j178/prefligit/releases/download/v0.0.22/prefligit-installer.ps1 | iex"
```
</details>

<details>
<summary>PyPI</summary>

prefligit is published as Python binary wheel to PyPI, you can install it using `pip`, `uv` (recommended), or `pipx`:

```console
pip install prefligit

# or

uv tool install prefligit

# or

pipx install prefligit
```
</details>

<details>
<summary>Homebrew</summary>

```console
brew install prefligit
```
</details>

<details>
<summary>Cargo</summary>

Build from source using Cargo:

```console
cargo install --locked --git https://github.com/j178/prefligit
```
</details>

<details>
<summary>GitHub Releases</summary>

prefligit release artifacts can be downloaded directly from the [GitHub releases](https://github.com/j178/prefligit/releases).
</details>

If installed via the standalone installer, prefligit can update itself to the latest version:

```console
$ prefligit self update
```

## Acknowledgements

This project is heavily inspired by the original [pre-commit](https://pre-commit.com/) tool, and it wouldn't be possible without the hard work
of the maintainers and contributors of that project.

And a special thanks to the [Astral](https://github.com/astral-sh) team for their remarkable projects, particularly [uv](https://github.com/astral-sh/uv),
from which I've learned a lot on how to write efficient and idiomatic Rust code.
