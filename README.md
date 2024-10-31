# pre-commit-rs

![Development Status](https://img.shields.io/badge/Development-Early_Stage-yellowgreen)
[![CI](https://github.com/j178/pre-commit-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/j178/pre-commit-rs/actions/workflows/ci.yml)

A reimplementation of the [pre-commit](https://pre-commit.com/) tool in Rust, providing a faster and dependency-free alternative.
It aims to be a drop-in replacement for the original tool while also providing some more advanced features.

> [!WARNING]
> This project is still in very early development, only a few of the original pre-commit features are implemented.

## Features

- A single binary with no dependencies, do not require Python or any other runtime.
- Improved performance in hook preparation and execution.
- Fully compatible with the original pre-commit configurations and hooks.
- (TODO) Built-in support for monorepos.
- (TODO) Built-in implementation of some common hooks.
- (TODO) Integration with [`uv`](https://github.com/astral-sh/uv) for managing Python tools and environments.

## Installation

```shell
# (Not available yet) Using brew
brew install j178/tap/pre-commit-rs

# Using cargo
cargo install --git https://github.com/j178/pre-commit-rs

# Using precompiled binaries from GitHub releases, go to:
https://github.com/j178/pre-commit-rs/releases
```

## Usage

This tool is designed to be a drop-in replacement for the original pre-commit tool, so you can use it with your existing configurations and hooks.

Please refer to the [official documentation](https://pre-commit.com/) for more information on how to configure and use pre-commit.

## Acknowledgements

This project is heavily inspired by the original [pre-commit](https://pre-commit.com/) tool, and it wouldn't be possible without the hard work
of the maintainers and contributors of that project.

And a special thanks to the [Astral](https://github.com/astral-sh) team for their remarkable projects, particularly [uv](https://github.com/astral-sh/uv),
from which I've learned a lot on how to write efficient and idiomatic Rust code.
