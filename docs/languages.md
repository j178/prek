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

    `language_version` is parsed as a version request. For languages that use semver requests, you can specify ranges (for example `^1.2`, `>=1.5, <2.0`). See [Configuration Reference](reference/configuration.md#language_version) for details.

Languages with managed toolchain downloads in prek today:

- [Python](#python)
- [Node](#node)
- [Bun](#bun)
- [Deno](#deno)
- [Golang](#golang)
- [Rust](#rust)
- [Ruby](#ruby)

Other supported languages rely on system installations and will fail if a matching toolchain is not available.

## Language details

Below is how prek handles each language (with notes when it differs from pre-commit).

### bun

**Status in prek:** ✅ Supported.

prek installs Bun hooks via `bun install` and runs the configured entry. The repository should contain a `package.json`. `entry` should match a provided bin name or be a Bun command. `additional_dependencies` are supported.

Bun hooks run without needing a pre-installed Bun runtime when toolchain download is available.

#### `language_version`

Supported formats:

- `default` or `system`
- `bun`, `bun@latest`
- `bun@1`, `1`
- `bun@1.1`, `1.1`
- `bun@1.1.0`, `1.1.0`
- Semver ranges like `>=1.0, <2.0`
- Absolute path to a Bun executable

!!! note "prek-only"

    Bun language support is a prek extension. pre-commit does not have native `bun` support.

### conda

**Status in prek:** Not supported yet.

Tracking: [#52](https://github.com/j178/prek/issues/52)

### coursier

**Status in prek:** Not supported yet.

Tracking: [#53](https://github.com/j178/prek/issues/53)

### dart

**Status in prek:** ✅ Supported.

prek runs Dart hooks with a system-installed `dart` executable.

Dart hooks can run plain Dart commands, repository scripts, or package
executables:

- `entry: dart --version`
- `entry: dart ./tool/hook.dart`
- `entry: dart run bin/hook.dart`
- `entry: my-package-executable`

If the hook repository contains `pubspec.yaml`, prek uses it to resolve package
dependencies and declared executables. `additional_dependencies` are supported
for both package hooks and standalone Dart scripts.

#### `pubspec.yaml` executables

For package hooks, executables declared in `pubspec.yaml` can be used as hook
entries. The executable key is the command name:

```yaml
name: my_dart_hooks

executables:
  my-hook:
  aliased-hook: tool/main
```

`my-hook` resolves to `bin/my-hook.dart`. `aliased-hook` resolves to
`bin/tool/main.dart`. Empty or null executable values use the executable key as
the entrypoint name, matching Dart's pub behavior.

#### `additional_dependencies`

Use `package` for the latest compatible version or `package:version` to pin a
version:

```yaml
repos:
  - repo: local
    hooks:
      - id: dart-hook
        name: Dart hook
        language: dart
        entry: dart ./bin/hook.dart
        additional_dependencies:
          - path
          - args:2.7.0
```

#### `language_version`

Dart does not support managed toolchain installation today. It uses the system
`dart` executable, and explicit Dart version requests are rejected.

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
    See [Environment Variable Reference](reference/environment-variables.md) for details.

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

**Status in prek:** ✅ Supported.

prek supports .NET SDK-based hooks. Hook entries run with a matching `dotnet` on the PATH, and tools specified in `additional_dependencies` are installed into an isolated hook environment via `dotnet tool install --tool-path`.

#### `language_version`

Supported formats:

- `default` or `system`
- `language_version: "8"` – the .NET 8.0 SDK channel
- `language_version: "8.0"` – the .NET 8.0 SDK channel
- `language_version: "8.0.100"` – exactly .NET SDK 8.0.100
- `language_version: "8.0.1xx"` – the .NET 8.0 SDK feature-band channel
- `language_version: "net8.0"` – TFM-style alias for the .NET 8.0 SDK channel
- `language_version: "net8.0.1xx"` – TFM-style alias for the .NET 8.0 SDK feature-band channel
- `language_version: "net10.0"` – TFM-style alias for the .NET 10.0 SDK channel
- `language_version: "lts"` – the latest LTS SDK channel
- `language_version: "sts"` – the latest STS SDK channel

prek first looks for a matching system-installed `dotnet`, then falls back to downloading the SDK via the official install script when downloads are allowed. Channel-style requests (`8`, `8.0`, `8.0.1xx`, `lts`, `sts`, `net8.0`) are resolved to a concrete SDK version at install time.

#### `additional_dependencies`

Tools are installed into the hook's isolated `tools/` directory. Specify them in `additional_dependencies` as either `package:version` (to pin a specific version) or just `package` (to install the latest available version):

```yaml
repos:
  - repo: https://github.com/example/csharpier-hook
    rev: v1.0.0
    hooks:
      - id: csharpier
        additional_dependencies:
          # Pin to a specific version
          - "csharpier:1.2.6"
          # Or install the latest version available
          - "dotnet-format"
```

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

**Status in prek:** ✅ Supported.

prek installs Haskell hooks via Cabal and runs the configured entry. Please ensure the repository contains a `.cabal` file or configured `additional_dependencies` for proper dependency management.

#### `language_version`

`language_version` is not supported for Haskell hooks yet. It uses the system `cabal` and `ghc` installations.

The hook `entry` should point at an executable installed by `cabal`.

### julia

**Status in prek:** ✅ Supported.

prek installs Julia hooks into an isolated environment using Julia's built-in package manager (`Pkg`).

The hook repository can include a `Project.toml` (or `JuliaProject.toml`) and optionally a `Manifest.toml` (or `JuliaManifest.toml`). If these files are present, prek will use them to instantiate the environment. If no project file is found, an empty one is created to ensure the environment is correctly initialized.

`additional_dependencies` are supported and will be added to the environment via `Pkg.add`.

#### `language_version`

`language_version` is not supported for Julia hooks yet. It uses the system `julia` installation.

The hook `entry` should be a path to a julia source file relative to the hook repository (optionally with arguments). It is executed using `julia --project=<env_path> --startup-file=no`.

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

**Status in prek:** ✅ Supported.

prek installs gems from a `*.gemspec` and runs executables declared in the gemspec. `additional_dependencies` are installed into the same isolated gemset.

#### `language_version`

Supported formats:

- `default` or `system`
- `3`, `3.3`, `3.3.6`
- `ruby-3`, `ruby-3.3`, `ruby-3.3.6`
- Semver ranges like `>=3.2, <4.0`
- Absolute path to a Ruby executable

!!! note "prek-only"

    prek can use system-installed Rubies, including a variety of common version managers. On some platforms, if the system search fails to find a suitable version matching `language_version`, it can then attempt to download one.

    Ruby interpreters are downloaded from those built by the `rv` project, and as such are limited in supported platform versions (currently limited to MacOS and Linux on x86_64 and ARM64). Older versions are also not available, with the oldest being 3.2.1. Unsupported platforms or versions will require a compatible system Ruby installation.

    The `PREK_RUBY_MIRROR` environment variable can be used to point to a different source for installers, for example to support mirrors or air-gapped CI environments. Mirrors need to follow the GitHub URL patterns, but note that although the GitHub hostname changes between `api.github.com` and `github.com` as needed, any non-GitHub mirror server will not be remapped in this manner. Where Ruby is being downloaded from GitHub (either from the upstream `rv` or a mirror), this remapping does occur, and any `GITHUB_TOKEN` will be sent with the requests. This both limits impact of rate limiting, and also allows a private GitHub repository to be used (e.g. for a vetted subset of `rv` rubies to be mirrored). Note that GitHub tokens will only be sent to mirrors which are hosted on GitHub.

Gems specified in hook gemspec files and `additional_dependencies` are installed into an isolated gemset shared across hooks with the same Ruby version and dependencies.

### rust

**Status in prek:** ✅ Supported.

prek installs binaries via `cargo install --bins --locked` and runs the specified executable. The repository should contain a `Cargo.toml` that produces the binary referenced by `entry`. `additional_dependencies` and `language_version` are supported.

!!! note "Using `--locked` flag"

    prek uses the `--locked` flag when installing Rust packages to ensure exact dependency versions from `Cargo.lock` are used. This prevents breaking changes from new dependency releases.

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
        - CLI dependencies using `cli:`.
            - There are two forms:
                - crates.io: `cli:<crate>[:<version>]`
                - git: `cli:<url>[:<tag>[:<package>]]`
            - For git dependencies:
                - `<url>` is the git repository URL.
                - `<tag>` is optional and selects a specific git tag.
                - `<package>` is optional and selects which Cargo package to install binaries from.
                - Use `<package>` when the git repository is a workspace or multi-crate repository and Cargo needs you to choose one package.
                - This matches the package argument in `cargo install --git <url> <package>`.
            - Examples:
                - crates.io package: `cli:rg`
                - crates.io package with version: `cli:rg:13.0.0`
                - git repository default ref: `cli:https://github.com/fish-shell/fish-shell`
                - git repository with tag: `cli:https://github.com/fish-shell/fish-shell:v4.5.0`
                - git repository with package but no tag: `cli:https://github.com/fish-shell/fish-shell::fish`
                - git repository with tag and package: `cli:https://github.com/fish-shell/fish-shell:v4.5.0:fish`
            - Invalid forms:
                - empty package is invalid, for example `...:v4.5.0:` or `...::`.

### swift

**Status in prek:** ✅ Supported.

prek detects the system Swift installation and runs hooks using the configured `entry`. If the hook repository contains a `Package.swift`, prek builds it in release mode and adds the resulting binaries to PATH.

Runtime behavior:

- Uses the system Swift installation (no automatic toolchain management)
- Builds Swift packages with `swift build -c release`
- Build artifacts are stored in the hook environment's `.build/release/` directory
- The `entry` command runs with built binaries available on PATH

#### `language_version`

Swift does not support `language_version` today. It uses the system `swift` installation.

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

**Status in prek:** ✅ Supported.

prek installs each `additional_dependencies` item with `deno install --global` into the hook environment. The hook runs from the work repository with an isolated `DENO_DIR` for cache separation.

Deno hooks run without needing a pre-installed Deno runtime when toolchain download is available.

#### Rules

- `additional_dependencies` are treated as executable installs. Each item should be something `deno install --global` can install, such as an `npm:` or `jsr:` specifier.
- `additional_dependencies` may also point at a local file to install as an executable, using `./path/to/tool.ts:name`. Relative paths resolve from the hook repository for remote hooks and from the work repository for local hooks.
- To override the executable name for an additional dependency, append `:name` to the dependency string. For example: `npm:semver@7:semver-tool`.

For remote hooks, if the repo wants to provide its own executable, declare it explicitly in the hook's `additional_dependencies`, for example `./cli.ts:repo-tool`, and then use `repo-tool` in `entry`.

#### `language_version`

Supported formats:

- `default` or `system`
- `deno`, `deno@latest`
- `deno@x`, `x` (major version)
- `deno@x.y`, `x.y` (major.minor version)
- `deno@x.y.z`, `x.y.z` (exact version)
- Semver ranges like `>=x.y, <x+1.0`

#### Using npm packages

Deno supports npm packages via the `npm:` prefix. For hooks that use npm packages, specify the entry using `deno run npm:package`:

```yaml
repos:
  - repo: local
    hooks:
      - id: eslint
        name: ESLint
        language: deno
        entry: deno run -A npm:eslint
        types: [ts, tsx, js, jsx]
```

For JSR packages, use the `jsr:` prefix in a `deno run` entry:

```yaml
repos:
  - repo: local
    hooks:
      - id: biome
        name: Biome
        language: deno
        entry: deno run -A jsr:@biomejs/biome
        types: [ts, tsx, js, jsx]
```

For executable-style additional dependencies, use the package specifier directly:

```yaml
repos:
  - repo: local
    hooks:
      - id: semver-version
        name: semver version
        language: deno
        entry: semver-tool 1.2.3
        additional_dependencies:
          - npm:semver@7:semver-tool
        pass_filenames: false
```

You can also install a local file as an executable additional dependency:

```yaml
repos:
  - repo: local
    hooks:
      - id: local-tool
        name: local tool
        language: deno
        entry: echo-tool
        additional_dependencies:
          - ./tool.ts:echo-tool
        pass_filenames: false
```

#### Built-in commands

Deno's built-in commands (`deno fmt`, `deno lint`, `deno check`) work directly:

```yaml
repos:
  - repo: local
    hooks:
      - id: deno-fmt
        name: Deno Format
        language: deno
        entry: deno fmt
        types: [ts, tsx, js, jsx, json, md]
      - id: deno-lint
        name: Deno Lint
        language: deno
        entry: deno lint
        types: [ts, tsx, js, jsx]
```

!!! note "prek-only"

    Deno language support is a prek extension. pre-commit does not have native `deno` support.

If you want to help add support for the missing languages, check open issues or start a discussion in the repo.
