# pre-commit-rs

![Development Status](https://img.shields.io/badge/Development-Early_Stage-yellowgreen)
[![CI](https://github.com/j178/pre-commit-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/j178/pre-commit-rs/actions/workflows/ci.yml)

A reimplementation of the [pre-commit](https://pre-commit.com/) tool in Rust, providing a faster and dependency-free alternative.
It aims to be a drop-in replacement for the original tool while also providing some more advanced features.

> [!WARNING]
> This project is still in very early development, only a few of the original pre-commit features are implemented.
> It is not recommended for normal use yet, but feel free to try it out and provide feedback.

## Features

- A single binary with no dependencies, do not require Python or any other runtime.
- Improved performance in hook preparation and execution.
- Fully compatible with the original pre-commit configurations and hooks.
- (TODO) Built-in support for monorepos.
- (TODO) Built-in implementation of some common hooks.
- (TODO) Integration with [`uv`](https://github.com/astral-sh/uv) for managing Python tools and environments.

## Installation

### Standalone installer

`pre-commit-rs` provides a standalone installer script to download and install the tool:

```console
# On Linux and macOS
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/j178/pre-commit-rs/releases/download/v0.0.3/pre-commit-rs-installer.sh | sh

# On Windows
powershell -ExecutionPolicy ByPass -c "irm https://github.com/j178/pre-commit-rs/releases/download/v0.0.3/pre-commit-rs-installer.ps1 | iex"
```

### PyPI

pre-commit-rs is published as Python binary wheel to PyPI under the name `pre-commit-rusty`,
you can install it using `pip`, `uv` (recommended), or `pipx`:

```console
pip install pre-commit-rusty

# or

uv tool install pre-commit-rusty

# or

pipx install pre-commit-rusty
```

### Homebrew

```console
brew install j178/tap/pre-commit-rs
```

### Cargo

Build from source using Cargo:

```console
cargo install --locked pre-commit-rs
```

Install from the binary directly using `cargo binstall`:

```console
cargo binstall pre-commit-rs
```

### GitHub Releases

`pre-commit-rs` release artifacts can be downloaded directly from the [GitHub releases](https://github.com/j178/pre-commit-rs/releases).

## Usage

> [!NOTE]
> The binary executable is named `pre-commit` (or `pre-commit.exe` on Windows) - without the `-rs` suffix. It should be available in your `PATH` after installation.

This tool is designed to be a drop-in replacement for the original pre-commit tool, so you can use it with your existing configurations and hooks.

Please refer to the [official documentation](https://pre-commit.com/) for more information on how to configure and use pre-commit.

## Acknowledgements

This project is heavily inspired by the original [pre-commit](https://pre-commit.com/) tool, and it wouldn't be possible without the hard work
of the maintainers and contributors of that project.

And a special thanks to the [Astral](https://github.com/astral-sh) team for their remarkable projects, particularly [uv](https://github.com/astral-sh/uv),
from which I've learned a lot on how to write efficient and idiomatic Rust code.
