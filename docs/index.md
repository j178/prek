# prek

<div align="center">
  <img width="220" alt="prek" src="/assets/logo.png" />
</div>

[pre-commit](https://pre-commit.com/) is a framework to run hooks written in many languages, and it manages the language toolchain and dependencies for running the hooks.

*prek* is a reimagined version of pre-commit, built in Rust. It is designed to be a faster, dependency-free and drop-in alternative for it, while also providing some additional long-requested features.

!!! warning "Not production-ready yet"
    prek is not production-ready yet. Some subcommands and languages are not implemented. See the current gaps for drop-in parity on the [TODO page](./todo.md).

    It's already being adopted by [some projects](#who-is-using-prek), please give it a try - we'd love your feedback!

## Features

- 🚀 A single binary with no dependencies, does not require Python or any other runtime.
- ⚡ About [10x faster](./benchmark.md) than `pre-commit` and uses only a third of disk space.
- 🔄 Fully compatible with the original pre-commit configurations and hooks.
- 🏗️ Built-in support for monorepos (i.e. [workspace mode](./workspace.md)).
- 🐍 Integration with [`uv`](https://github.com/astral-sh/uv) for managing Python virtual environments and dependencies.
- 🛠️ Improved toolchain installations for Python, Node.js, Go, Rust and Ruby, shared between hooks.
- 📦 Built-in implementation of some common hooks.

## Quick Start

1. [Install prek](./installation.md)
2. Replace `pre-commit` with `prek` in your commands
3. Your existing `.pre-commit-config.yaml` works unchanged

    ```console
    $ prek run
    trim trailing whitespace.................................................Passed
    fix end of files.........................................................Passed
    typos....................................................................Passed
    cargo fmt................................................................Passed
    cargo clippy.............................................................Passed
    ```

## Why prek?

### prek is way faster

- It is about [10x faster](./benchmark.md) than `pre-commit` and uses only a third of disk space.
- It redesigned how hook environments and toolchains are managed, they are all shared between hooks, which reduces the disk space usage and speeds up the installation process.
- Repositories are cloned in parallel, and hooks are installed in parallel if their dependencies are disjoint.
- It uses [`uv`](https://github.com/astral-sh/uv) for creating Python virtualenvs and installing dependencies, which is known for its speed and efficiency.
- It implements some common hooks in Rust, built in prek, which are faster than their Python counterparts.

### prek provides a better user experience

- No need to install Python or any other runtime, just download a single binary.
- No hassle with your Python version or virtual environments, prek automatically installs the required Python version and creates a virtual environment for you.
- Built-in support for workspaces (or monorepos), each subproject can have its own `.pre-commit-config.yaml` file.
- `prek run` has some nifty improvements over `pre-commit run`, such as:
  - `prek run --directory <dir>` runs hooks for files in the specified directory, no need to use `git ls-files -- <dir> | xargs pre-commit run --files` anymore.
  - `prek run --last-commit` runs hooks for files changed in the last commit.
  - `prek run [HOOK] [HOOK]` selects and runs multiple hooks.
- `prek list` command lists all available hooks, their ids, and descriptions, providing a better overview of the configured hooks.
- prek provides shell completions for `prek run <hook_id>` command, making it easier to run specific hooks without remembering their ids.

For more detailed improvements prek offers, take a look at [Difference from pre-commit](./diff.md).

## Who is using prek?

prek is pretty new, but it is already being used or recommend by some projects and organizations:

- [Airflow](https://github.com/apache/airflow/issues/44995)
- [PDM](https://github.com/pdm-project/pdm/pull/3593)
- [basedpyright](https://github.com/DetachHead/basedpyright/pull/1413)
- [OpenLineage](https://github.com/OpenLineage/OpenLineage/pull/3965)
- [Authlib](https://github.com/authlib/authlib/pull/804)

## Getting Started

- [Installation](./installation.md) - Installation options
- [Workspace Mode](./workspace.md) - Monorepo support
- [Differences](./diff.md) - What's different from pre-commit
- [Debugging](./debugging.md) - Troubleshooting tips
