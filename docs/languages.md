# Language support

## What “language” means in prek

Each hook has a `language` that tells prek how to install and run it. The language determines:

- Whether prek creates a managed environment for the hook
- How dependencies are installed (`additional_dependencies`)
- How toolchain versions are selected (`language_version`)
- How `entry` is executed

For `repo: local` hooks, `language` is required. For remote hooks, it is read from `.pre-commit-hooks.yaml`, but you can override it in your config.

## Toolchain management and `language_version`

prek resolves toolchains in two steps:

1. **Discover system toolchains** (PATH and common version manager locations).
2. **Download a toolchain** when the language supports it and the request cannot be satisfied locally.

If `language_version` is `system`, prek skips downloads and requires a system-installed toolchain. If `language_version` is `default`, prek uses the language’s default resolution logic (often preferring system installs, then downloading if supported).

!!! note "prek-only"

    `language_version` is parsed as a version request. For languages that use semver requests, you can specify ranges (for example `^1.2`, `>=1.5, <2.0`). See [Configuration](configuration.md#language_version) for details.

Languages with managed toolchain downloads in prek today:

- [Python](#python)
- [Node](#node)
- [Golang](#golang)
- [Rust](#rust)

Other supported languages rely on system installations and will fail if a matching toolchain is not available.

## Language details

Below is how prek handles each language (with notes when it differs from pre-commit).

### conda

**Status in prek:** Not supported yet.

Tracking: [#52](https://github.com/j178/prek/issues/52)

### coursier

**Status in prek:** Not supported yet.

Tracking: [#53](https://github.com/j178/prek/issues/53)

### dart

**Status in prek:** Not supported yet.

Tracking: [#51](https://github.com/j178/prek/issues/51)

### docker

**Status in prek:** ✅ Supported.

prek expects the hook repository to ship a Dockerfile and builds the image from the repo root with `docker build .`. Hooks run inside the container, and the first token of `entry` is used as the container entrypoint (arguments are passed after it).

Runtime behavior:

- Requires a working container engine on the host (Docker, Podman, or Container).
- The repository is bind-mounted into the container at `/src` and the working directory is set to `/src`.
- The container is run with `--entrypoint` set to the hook `entry`, so the image’s default command is not used when filenames are passed.
- Environment variables configured via `env` are passed using `-e`.
- On Linux, prek tries to run as a non-root user and handles rootless Podman with `--userns=keep-id`.

Use `docker` when you need a language runtime that isn’t otherwise supported; the container provides the execution environment.

!!! note "prek-only"

    prek auto-detects the container runtime (Docker, Podman, or [Container](https://github.com/apple/container)) and can be overridden with `PREK_CONTAINER_RUNTIME`.
    See [Configuration](configuration.md#environment-variables) for details.

### docker_image

**Status in prek:** ✅ Supported.

prek runs hooks from an existing image. The `entry` value is passed to `docker run` directly, so it should include the image reference and can optionally include `--entrypoint` overrides.

Runtime behavior:

- Uses the same bind-mount and `/src` working directory as `docker` hooks.
- Environment variables configured via `env` are passed using `-e`.

If the image already defines an `ENTRYPOINT`, you can omit `--entrypoint` in `entry`. Otherwise, specify it explicitly in `entry`.

!!! note "prek-only"

    prek uses the same runtime auto-detection as `docker` hooks.

### dotnet

**Status in prek:** Not supported yet.

Tracking: [#48](https://github.com/j178/prek/issues/48)

### fail

**Status in prek:** ✅ Supported.

`fail` is a lightweight “forbid files” hook. The `entry` text is printed when the hook fails, followed by the list of matching files, and the hook exits non-zero.

### golang

**Status in prek:** ✅ Supported.

prek installs with `go install ./...` in an isolated `GOPATH`. The repository should build at least one binary whose name matches the hook `entry`. `additional_dependencies` can be appended and `language_version` selects the Go toolchain.

#### `language_version`

Supported formats:

- `default` or `system`
- `go1.22`, `1.22`
- `go1.22.1`, `1.22.1`
- Semver ranges like `>=1.20, <1.23`
- Absolute path to a `go` executable

Pre-release strings (for example `go1.22rc1`) are not supported yet.

### haskell

**Status in prek:** Not supported yet.

Tracking: [#1445](https://github.com/j178/prek/issues/1445)

### julia

**Status in prek:** Not supported yet.

Tracking: [#1446](https://github.com/j178/prek/issues/1446)

### lua

**Status in prek:** ✅ Supported.

prek installs Lua hooks via LuaRocks and runs the configured entry. If the repository includes a rockspec, it is installed into the hook environment before running.

#### `language_version`

Lua does not support `language_version` today. It uses the system `lua` / `luarocks` installation.

The hook entry should point at an executable installed by LuaRocks.

### node

**Status in prek:** ✅ Supported.

prek expects a `package.json` and installs via `npm install .`, exposing executables from the package `bin`. `entry` should match a provided bin name. `additional_dependencies` are supported.

Node hooks run without needing a pre-installed Node runtime when toolchain download is available.

#### `language_version`

Supported formats:

- `default` or `system`
- `node18`, `18`, `18.19`, `18.19.1`
- Semver ranges like `^18.12` or `>=18, <20`
- LTS selectors: `lts` or `lts/<codename>`
- Absolute path to a Node executable

### perl

**Status in prek:** Not supported yet.

Tracking: [#1447](https://github.com/j178/prek/issues/1447)

### python

**Status in prek:** ✅ Supported.

prek installs hook repositories with `uv pip install` and uses the installed console scripts. The repository should be installable via `pip` (for example via `pyproject.toml` or `setup.py`). `additional_dependencies` are appended to the install step.

Python hooks run without requiring a system Python when toolchain download is available.

#### `language_version`

Supported formats:

- `default` or `system`
- `python`, `python3`, `python3.12`, `python3.12.1`
- `3`, `3.12`, `3.12.1`
- Wheel-style short forms like `312` or `python312`
- Semver ranges like `>=3.9, <3.13`
- Absolute path to a Python executable

!!! note "prek-only"

    prek uses `uv` for virtual environments and dependency installs, and can auto-install Python toolchains based on `language_version`.

#### Dependency management with `uv`

prek uses `uv` for creating virtual environments and installing dependencies:

- First tries to find `uv` in the system PATH
- If not found, automatically installs `uv` from the best available source (GitHub releases, PyPI, or mirrors)
- Automatically installs the required Python version if it's not already available

!!! warning "Environment variables"

    Since prek calls `uv` under the hood to create Python virtual environments and install dependencies, most `uv` environment variables will affect prek's behavior. For example, setting `UV_RESOLUTION=lowest-direct` in your environment will cause hook dependencies to be resolved to their lowest compatible versions, which may lead to installation failures with old packages on modern Python versions.

    If you encounter unexpected behavior when installing Python hooks, check whether you have any `UV_*` environment variables set that might be affecting dependency resolution or installation.

#### PEP 723 inline script metadata support

For Python hooks **without** `additional_dependencies`, prek can read PEP 723 inline metadata from the script specified in the `entry` field.

**Example:**

`.pre-commit-config.yaml`:

```yaml
repos:
  - repo: local
    hooks:
      - id: echo
        name: echo
        language: python
        entry: ./echo.py
```

`echo.py`:

```python
# /// script
# requires-python = ">=3.13"
# dependencies = [
#     "pyecho-cli",
# ]
# ///

from pyecho import main
main()
```

**Important notes:**

- The first part of the `entry` field must be a path to a local Python script
- If `additional_dependencies` is specified in `.pre-commit-config.yaml`, script metadata will be ignored
- When both `language_version` (in config) and `requires-python` (in script) are set, `language_version` takes precedence
- Only `dependencies` and `requires-python` fields are supported; other metadata like `tool.uv` is ignored

### r

**Status in prek:** Not supported yet.

Tracking: [#42](https://github.com/j178/prek/issues/42)

### ruby

**Status in prek:** ✅ Supported (with limitations).

prek installs gems from a `*.gemspec` and runs executables declared in the gemspec. `additional_dependencies` are installed into the same isolated gemset.

#### `language_version`

Supported formats:

- `default` or `system`
- `3`, `3.3`, `3.3.6`
- `ruby-3`, `ruby-3.3`, `ruby-3.3.6`
- Semver ranges like `>=3.2, <4.0`
- Absolute path to a Ruby executable

!!! note "prek-only"

    prek does not automatically download Ruby toolchains. It uses system-installed Rubies, including common version managers, and fails if no suitable version matches `language_version`.

Tracking for Ruby toolchain download support: [#43](https://github.com/j178/prek/issues/43)

Gems specified in hook gemspec files and `additional_dependencies` are installed into an isolated gemset shared across hooks with the same Ruby version and dependencies.

### rust

**Status in prek:** ✅ Supported.

prek installs binaries via `cargo install --bins` and runs the specified executable. The repository should contain a `Cargo.toml` that produces the binary referenced by `entry`. `additional_dependencies` and `language_version` are supported.

#### `language_version`

Supported formats:

- `default` or `system`
- Channels: `stable`, `beta`, `nightly`
- `1`, `1.70`, `1.70.0`
- Semver ranges like `>=1.70, <1.72`

!!! note "prek-only"

    - prek supports installing packages from virtual workspaces. See [#1180](https://github.com/j178/prek/pull/1180).
    - `additional_dependencies` supports:
        - Library dependencies using `name` or `name:version` (applied via `cargo add`).
        - CLI dependencies using `cli:`. These can be crates.io packages (`cli:rg:13.0.0`) or git URLs (`cli:https://github.com/BurntSushi/ripgrep:13.0.0`).

### swift

**Status in prek:** Not supported yet.

Tracking: [#46](https://github.com/j178/prek/issues/46)

### pygrep

**Status in prek:** ✅ Supported.

prek provides a Python-based grep implementation for file content matching. The `entry` is a Python regex. Supported args:

- `-i` / `--ignore-case`
- `--multiline`
- `--negate` (require all files to match)

Regex matching uses Python’s `re` semantics for compatibility with pre-commit.

### system

**Status in prek:** ✅ Supported.

`system` runs a system executable without a managed environment. The command is taken from `entry`, and filenames are appended unless `pass_filenames: false` is set. Dependencies must be installed by the user.

Use `system` for tools with special environment requirements that cannot run in isolated environments.

!!! note

    `unsupported` is accepted as an alias for `system`.

### script

**Status in prek:** ✅ Supported.

`script` runs repository-local scripts without a managed environment. For remote hooks, `entry` is resolved relative to the hook repository root; for local hooks, it is resolved relative to the current working directory.

Use `script` for simple repository scripts that only need file paths and no managed environment.

!!! note

    `unsupported_script` is accepted as an alias for `script`.

### deno

**Status in prek:** 🚧 WIP.

prek has experimental support in progress. pre-commit does not have a native `deno` language.

Tracking: [#619](https://github.com/j178/prek/issues/619)

If you want to help add support for the missing languages, check open issues or start a discussion in the repo.
