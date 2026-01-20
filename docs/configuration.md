# Configuration

`prek` is **fully compatible** with the [`pre-commit`](https://pre-commit.com/) configuration file `.pre-commit-config.yaml`, so your existing configs work unchanged.

`prek` uses the same configuration model as [`pre-commit`](https://pre-commit.com/): you declare repositories under `repos:`, then enable and configure hooks from those repositories.

In addition to compatibility, `prek` adds a few extra keys (documented here) for features like workspace mode and parallel scheduling.

## Configuration file

### Location (discovery)

By default, `prek` looks for a configuration file starting from your current working directory and moving upward.
It stops when it finds a config file, or when it hits the git repository boundary.

If you run **without** `--config`, `prek` then enables **workspace mode**:

- The first config found while traversing upward becomes the workspace root.
- From that root, `prek` searches for additional config files in subdirectories (nested projects).

Workspace discovery respects `.gitignore`, and also supports `.prekignore` for excluding directories from discovery.
For the full behavior and examples, see [Workspace Mode](workspace.md).

If you pass `--config` / `-c`, workspace discovery is disabled and only that single config file is used.

### File name

`prek` recognizes the following configuration filenames:

- `.pre-commit-config.yaml` (preferred)
- `.pre-commit-config.yml` (alternate)

In workspace mode, each project uses one of these filenames in its own directory.

### File format

The configuration file is YAML.

Practical notes:

- Keys are typically `snake_case`.
- Regular expressions are provided as YAML strings.
  If your regex contains backslashes, quote it (e.g. `files: '\\.rs$'`).
- YAML anchors/aliases and merge keys are supported, so you can de-duplicate repeated blocks.

Example (small but complete):

```yaml
default_language_version:
  python: python3.12

repos:
  - repo: local
    hooks:
      - id: ruff
        name: ruff
        language: system
        entry: python3 -m ruff check
        files: '\\.py$'
```

### Scope (per-project)

Each `.pre-commit-config.yaml` / `.pre-commit-config.yml` file is scoped to the **project directory it lives in**.

In workspace mode, `prek` treats every discovered configuration file as a **distinct project**:

- A project’s config only controls hook selection and filtering (for example `files` / `exclude`) for that project.
- A project may contain nested subprojects (subdirectories with their own config). Those subprojects run using *their own* configs.

Practical implication: filters in the parent project do not “turn off” a subproject.

Example layout (monorepo with a nested project):

- `foo/.pre-commit-config.yaml` (project `foo`)
- `foo/bar/.pre-commit-config.yaml` (project `foo/bar`, nested subproject)

If project `foo` config contains an `exclude` that matches `bar/**`, then hooks for project `foo` will not run on files under `foo/bar`:

```yaml
# foo/.pre-commit-config.yaml
exclude:
  glob: bar/**
```

But if `foo/bar` is itself a project (has its own config), files under `foo/bar` are still eligible for hooks when running **in the context of project `foo/bar`**.

!!! note "Excluding a nested project"

    If `foo/bar/.pre-commit-config.yaml` exists but you *don’t* want it to be recognized as a project in workspace mode, exclude it from discovery using [`.prekignore`](workspace.md#discovery).

    Like `.gitignore`, `.prekignore` files can be placed anywhere in the workspace and apply to their directory and all subdirectories.

### Validation

Use `prek validate-config` to validate one or more config files.

If you want IDE completion / validation, the repository ships a JSON schema in `prek.schema.json`.
That schema tracks what `prek` accepts today, but `prek` also intentionally tolerates unknown keys for forward compatibility.

## Configuration reference

This section documents the keys `prek` supports in `.pre-commit-config.yaml` / `.pre-commit-config.yml`.

### Prek-only options

The following configuration entries are extensions implemented by `prek` and are not part of the documented `pre-commit` configuration surface.
If you run them with upstream `pre-commit`, expect them to be ignored or to trigger an “unknown key” warning.

- Top-level:

    - [`minimum_prek_version`](#prek-only-minimum-prek-version-config)
    - [`orphan`](#prek-only-orphan)

- Repo type:

    - [`repo: builtin`](#prek-only-repo-builtin)

- Hook-level:

    - [`env`](#prek-only-env)
    - [`priority`](#prek-only-priority)
    - [`minimum_prek_version`](#prek-only-minimum-prek-version-hook)

### Top-level keys

#### `repos` (required)

A list of hook repositories.

Each entry is one of:

- a remote repository (typically a git URL)
- `repo: local` for hooks defined directly in your repository
- `repo: meta` for built-in meta hooks
- `repo: builtin` for `prek`'s built-in fast hooks

See [Repo entries](#repo-entries).

<a id="top-level-files"></a>

#### `files`

Global *include* regex applied before hook-level filtering.

- Type: regex string (default, pre-commit compatible) **or** a prek-only glob pattern mapping
- Default: no global include filter

This is usually used to narrow down the universe of files in large repositories.

!!! note "prek-only globs"

    In addition to regex strings, `prek` supports glob patterns via:

    - `files: { glob: "..." }` (single glob)
    - `files: { glob: ["...", "..."] }` (glob list)

    This is a `prek` extension. Upstream `pre-commit` expects regex strings here.

    For more information on the glob syntax, refer to the [globset documentation](https://docs.rs/globset/latest/globset/#syntax).

Examples:

```yaml
# Regex (portable to pre-commit)
files: '\\.rs$'

# Glob (prek-only)
files:
  glob: src/**/*.rs

# Glob list (prek-only; matches if any glob matches)
files:
  glob:
    - src/**/*.rs
    - crates/**/src/**/*.rs
```

<a id="top-level-exclude"></a>

#### `exclude`

Global *exclude* regex applied before hook-level filtering.

- Type: regex string (default, pre-commit compatible) **or** a prek-only glob pattern mapping
- Default: no global exclude filter

`exclude` is useful for generated folders, vendored code, or build outputs.

!!! note "prek-only globs"

    Like `files`, `exclude` supports `glob` (single glob or glob list) as a `prek` extension.

    For more information on the glob syntax, refer to the [globset documentation](https://docs.rs/globset/latest/globset/#syntax).

Examples:

```yaml
# Regex (portable to pre-commit)
exclude: '^target/'

# Glob (prek-only)
exclude:
  glob: target/**

# Glob list (prek-only)
exclude:
  glob:
    - target/**
    - dist/**
```

#### `fail_fast`

Stop the run after the first failing hook.

- Type: boolean
- Default: `false`

This is a global default; individual hooks can also set `fail_fast`.

#### `default_language_version`

Map a language name to the default `language_version` used by hooks of that language.

- Type: map
- Default: none (hooks fall back to `language_version: default`)

Example:

```yaml
default_language_version:
  python: python3.12
  node: '20'
```

`prek` treats `language_version` as a version request (often a semver-like selector) and may install toolchains automatically. See [Difference from pre-commit](diff.md).

#### `default_stages`

Default `stages` used when a hook does not specify its own.

- Type: list of stage names
- Default: all stages

Allowed values:

- `manual`
- `commit-msg`
- `post-checkout`
- `post-commit`
- `post-merge`
- `post-rewrite`
- `pre-commit`
- `pre-merge-commit`
- `pre-push`
- `pre-rebase`
- `prepare-commit-msg`

#### `default_install_hook_types`

Default hook type(s) installed by `prek install` when you don’t pass `--hook-type`.

- Type: list of git hook types
- Default: `[pre-commit]`

This controls which *git hook scripts* are installed (for example `pre-commit` vs `pre-push`).
It is separate from a hook’s `stages`, which controls when a particular hook is eligible to run.

Allowed values:

- `pre-commit`
- `pre-push`
- `commit-msg`
- `prepare-commit-msg`
- `post-checkout`
- `post-commit`
- `post-merge`
- `post-rewrite`
- `pre-merge-commit`
- `pre-rebase`

#### `minimum_prek_version`

<a id="prek-only-minimum-prek-version-config"></a>

!!! note "prek-only"

    This key is a `prek` extension. Upstream `pre-commit` uses `minimum_pre_commit_version`, which `prek` intentionally ignores.

Require a minimum `prek` version for this config.

- Type: string (version)
- Default: unset

If the installed `prek` is older than the configured minimum, `prek` exits with an error.

Example:

```yaml
minimum_prek_version: '0.2.0'
```

#### `orphan`

<a id="prek-only-orphan"></a>

!!! note "prek-only"

    `orphan` is a `prek` workspace-mode feature and is not recognized by upstream `pre-commit`.

Workspace-mode setting to isolate a nested project from parent configs.

- Type: boolean
- Default: `false`

When `orphan: true`, files under this project directory are handled only by this project’s config and are not “seen” by parent projects.

Example:

```yaml
orphan: true
repos:
  - repo: https://github.com/astral-sh/ruff-pre-commit
    rev: v0.8.4
    hooks:
      - id: ruff
```

See [Workspace Mode - File Processing Behavior](workspace.md#file-processing-behavior) for details.

### Repo entries

Each item under `repos:` is a mapping that always contains a `repo:` key.

#### Remote repository

Use this for hooks distributed in a separate repository.

Required keys:

- `repo`: repository location (commonly an https git URL)
- `rev`: version to use (tag, branch, or commit SHA)
- `hooks`: list of hook selections

##### `repo`

Where to fetch hooks from.

In most configs this is a git URL.
`prek` also recognizes special values documented separately: `local`, `meta`, and `builtin`.

##### `rev`

The revision to use for the remote repository.

Use a tag or commit SHA for repeatable results.
If you use a moving target (like a branch name), runs may change over time.

##### `hooks`

The list of hooks to enable from that repository.

Each item must at least specify `id`.
You can also add hook-level options (filters, args, stages, etc.) to customize behavior.

Example:

```yaml
repos:
  - repo: https://github.com/astral-sh/ruff-pre-commit
    rev: v0.8.4
    hooks:
      - id: ruff
        args: [--fix]
```

Notes:

- For reproducibility, prefer immutable pins (tags or commit SHAs).
- `prek auto-update` can help update `rev` values.

#### `repo: local`

Define hooks inline inside your repository.

Keys:

- `repo`: must be `local`
- `hooks`: list of **local hook definitions** (see [Local hook definition](#local-hook-definition))

Example:

```yaml
repos:
  - repo: local
    hooks:
      - id: cargo-fmt
        name: cargo fmt
        language: system
        entry: cargo fmt
        files: '\\.rs$'
```

#### `repo: meta`

Use `pre-commit`-style meta hooks that validate and debug your configuration.

`prek` supports the following meta hook ids:

- `check-hooks-apply`
- `check-useless-excludes`
- `identity`

Restrictions:

- `id` is required.
- `entry` is not allowed.
- `language` (if set) must be `system`.

You may still configure normal hook options such as `files`, `exclude`, `stages`, etc.

Example:

```yaml
repos:
  - repo: meta
    hooks:
      - id: check-useless-excludes
```

#### `repo: builtin`

<a id="prek-only-repo-builtin"></a>

!!! note "prek-only"

    `repo: builtin` is specific to `prek` and is not compatible with upstream `pre-commit`.

Use `prek`’s built-in fast hooks (offline, zero setup).

Restrictions:

- `id` is required.
- `entry` is not allowed.
- `language` (if set) must be `system`.

Example:

```yaml
repos:
  - repo: builtin
    hooks:
      - id: trailing-whitespace
      - id: check-yaml
```

For the list of available built-in hooks and the “automatic fast path” behavior, see [Built-in Fast Hooks](builtin.md).

### Hook entries

Hook items under `repos[*].hooks` have slightly different shapes depending on the repo type.

#### Remote hook selection

For a remote repo, the hook entry must include:

- `id` (required): selects the hook from the repository

All other hook keys are optional overrides (for example `args`, `files`, `exclude`, `stages`, …).

!!! note "Advanced overrides"

    `prek` also supports overriding `name`, `entry`, and `language` for remote hooks.
    This can be useful for experimentation, but it may reduce portability to the original `pre-commit`.

#### Local hook definition

For `repo: local`, the hook entry is a full definition and must include:

- `id` (required): stable identifier used by `prek run <id>` and selectors
- `name` (required): label shown in output
- `entry` (required): command to execute
- `language` (required): how `prek` sets up and runs the hook

#### Builtin/meta hook selection

For `repo: builtin` and `repo: meta`, the hook entry must include `id`.
You can optionally provide `name` and normal hook options (filters, stages, etc), but not `entry`.

### Common hook options

These keys can appear on hooks (remote/local/builtin/meta), subject to the restrictions above.

#### `id`

The stable identifier of the hook.

- For remote hooks, this must match a hook id defined by the remote repository.
- For local hooks, you choose it.

`id` is also used for CLI selection (for example `prek run <id>` and `PREK_SKIP`).

#### `name`

Human-friendly label shown in output.

- Required for `repo: local` hooks.
- Optional as an override for remote/meta/builtin hooks.

#### `entry`

The command line to execute for the hook.

- Required for `repo: local` hooks.
- Optional override for remote hooks.
- Not allowed for `repo: meta` and `repo: builtin`.

If `pass_filenames: true`, `prek` appends matching filenames to this command when running.

#### `language`

How `prek` should run the hook (and whether it should create a managed environment).

- Required for `repo: local` hooks.
- Optional override for remote hooks.
- Not allowed (except as `system`) for `repo: meta` and `repo: builtin`.

Common values include `system`, `python`, `node`, `rust`, `golang`, `ruby`, and `docker`.

See [Language Support](languages.md) for per-language behavior, supported values, and `language_version` details.

!!! note "Language name aliases"

    For compatibility with upstream `pre-commit`, the following legacy language names are also accepted:

    - `unsupported` is treated as `system`
    - `unsupported_script` is treated as `script`

#### `alias`

An alternate identifier for selecting the hook from the CLI.

If set, you can run the hook via either `prek run <id>` or `prek run <alias>`.

#### `args`

Extra arguments appended to the hook’s `entry`.

- Type: list of strings

Example:

```yaml
hooks:
  - id: ruff
    args: [--fix]
```

#### `env`

<a id="prek-only-env"></a>

!!! note "prek-only"

    `env` is a `prek` extension and may not be recognized by upstream `pre-commit`.

Extra environment variables for the hook process.

- Type: map of string to string

Values override the existing process environment (including variables such as `PATH`).

For `docker` / `docker_image` hooks, these variables are passed into the container rather than being applied to the container runtime command.

Example:

```yaml
repos:
  - repo: local
    hooks:
      - id: cargo-doc
        name: cargo doc
        language: system
        entry: cargo doc --all-features --workspace --no-deps
        env:
          RUSTDOCFLAGS: -Dwarnings
        pass_filenames: false
```

#### `files` / `exclude`

Filters applied to candidate filenames.

- `files` selects which files are eligible for the hook.
- `exclude` removes files matched by `files`.

If you use both global and hook-level filters, the effective behavior is “global filter first, then hook filter”.

By default (and for compatibility with upstream `pre-commit`), these are regex strings.
As a `prek` extension, you can also specify globs using `glob` or a glob list.

See [Top-level `files`](#top-level-files) and [Top-level `exclude`](#top-level-exclude) for the full syntax and examples.

#### `types` / `types_or` / `exclude_types`

File-type filters based on `identify` tags.

- `types`: all listed tags must match (logical AND)
- `types_or`: at least one listed tag must match (logical OR)
- `exclude_types`: tags that disqualify a file

Defaults:

- `types`: `[file]` (matches all files)
- `types_or`: `[]`
- `exclude_types`: `[]`

These filters are applied in addition to regex filtering.

#### `always_run`

Run the hook even when no files match.

- Type: boolean
- Default: `false`

This is commonly used for hooks that check repository-wide state (for example, running a test suite) rather than operating on specific files.

#### `pass_filenames`

Controls whether `prek` appends the matching filenames to the command line.

- Type: boolean
- Default: `true`

Set `pass_filenames: false` for hooks that don’t accept file arguments (or that discover files themselves).

#### `stages`

Declare which stages a hook is eligible to run in.

- Type: list of stage names
- Default: all stages

Allowed values:

- `manual`
- `commit-msg`
- `post-checkout`
- `post-commit`
- `post-merge`
- `post-rewrite`
- `pre-commit`
- `pre-merge-commit`
- `pre-push`
- `pre-rebase`
- `prepare-commit-msg`

When you run `prek run --hook-stage <stage>`, only hooks configured for that stage are considered.

#### `require_serial`

Force a hook to run without parallel invocations (one in-flight process for that hook at a time).

- Type: boolean
- Default: `false`

This is useful for tools that use global caches/locks or otherwise can’t handle concurrent execution.

#### `priority`

<a id="prek-only-priority"></a>

!!! note "prek-only"

    `priority` controls `prek`'s scheduler and does not exist in upstream `pre-commit`.

Each hook can set an explicit `priority` (a non-negative integer) that controls when it runs and with which hooks it may execute in parallel.

Scope:

- `priority` is evaluated **within a single configuration file** and is compared across **all hooks in that file**, even if they appear under different `repos:` entries.
- `priority` does **not** coordinate across different config files. In workspace mode, each project’s config file is scheduled independently.

Hooks run in ascending priority order: **lower `priority` values run earlier**. Hooks that share the same `priority` value run concurrently, subject to the global concurrency limit.

When `priority` is omitted, `prek` assigns an implicit value based on hook order to preserve sequential behavior.

Example:

```yaml
repos:
  - repo: local
    hooks:
      - id: format
        name: Format
        language: system
        entry: python3 -m ruff format
        always_run: true
        priority: 0

      - id: lint
        name: Lint
        language: system
        entry: python3 -m ruff check
        always_run: true
        priority: 10

      - id: tests
        name: Tests
        language: system
        entry: just test
        always_run: true
        priority: 20
```

!!! danger "Parallel hooks modifying files"

    If two hooks run in the same priority group and both mutate the same files (or depend on shared state), results are undefined.
    Use separate priorities to avoid overlap.

!!! note "`require_serial` is different"

    `require_serial: true` prevents concurrent invocations of the *same hook*.
    It does not prevent other hooks from running alongside it; use a unique `priority` if you need exclusivity.

#### `fail_fast`

Hook-level fail-fast behavior.

- Type: boolean
- Default: `false`

If `true`, a failure in this hook stops the run immediately.

#### `verbose`

Print hook output even when the hook succeeds.

- Type: boolean
- Default: `false`

#### `log_file`

Write hook output to a file when the hook fails (and also when `verbose: true`).

- Type: string path

#### `description`

Free-form description shown in listings / metadata.

- Type: string

#### `language_version`

Choose the language/toolchain version request for this hook.

- Type: string
- Default: `default`

If not set, `prek` may use `default_language_version` for the hook’s language.

!!! note "prek-only"

    `language_version` is treated as a **version request**, not a single pinned value. For languages that use semver requests, you can specify ranges (for example `^1.2`, `>=1.5, <2.0`).

    Special values:

    - `default`: use the language’s default resolution logic.
    - `system`: require a system-installed toolchain (no downloads).

    Language-specific behavior:

    - Python: passed to the Python resolver (for example `python3`, `python3.12`, or a specific interpreter name). May trigger toolchain download.
    - Node: passed to the Node resolver (for example `20`, `18.19.0`). May trigger toolchain download.
    - Go: uses Go version strings such as `1.22.1` (downloaded if missing).
    - Rust: supports rustup toolchains such as `stable`, `beta`, `nightly`, or versioned toolchains.
    - Other languages: parsed as a semver request and matched against the installed toolchain version.

    Examples:

    ```yaml
    hooks:
      - id: ruff
        language: python
        language_version: python3.12

      - id: eslint
        language: node
        language_version: '20'

      - id: cargo-fmt
        language: rust
        language_version: stable

      - id: my-tool
        language: system
        language_version: system
    ```

#### `additional_dependencies`

Extra dependencies for hooks that run inside a managed environment (for example Python or Node hooks).

- Type: list of strings

If you set this for a language that doesn’t support dependency installation, `prek` fails with a configuration error.

#### `minimum_prek_version`

<a id="prek-only-minimum-prek-version-hook"></a>

!!! note "prek-only"

    This is a `prek`-specific requirement gate. Upstream `pre-commit` does not have a hook-level minimum version key.

Require a minimum `prek` version for this specific hook.

- Type: string (version)
- Default: unset

## Environment variables

Prek supports the following environment variables:

- `PREK_HOME` — Override the prek data directory (caches, toolchains, hook envs). If beginning with `~`, it is expanded to the user’s home directory. Defaults to `~/.cache/prek` on macOS and Linux, and `%LOCALAPPDATA%\prek` on Windows.

- `PREK_COLOR` — Control colored output: auto (default), always, or never.

- `PREK_SKIP` — Comma-separated list of hook IDs to skip (e.g. black,ruff). See [Skipping Projects or Hooks](workspace.md#skipping-projects-or-hooks) for details.

- `PREK_ALLOW_NO_CONFIG` — Allow running without a .pre-commit-config.yaml (useful for ad‑hoc runs).

- `PREK_NO_CONCURRENCY` — Disable parallelism for installs and runs (set `PREK_NO_CONCURRENCY=1` to force concurrency to `1`).

- `PREK_NO_FAST_PATH` — Disable Rust-native built-in hooks; always use the original hook implementation. See [Built-in Fast Hooks](builtin.md) for details.

- `PREK_UV_SOURCE` — Control how uv (Python package installer) is installed. Options:

    - `github` (download from GitHub releases)
    - `pypi` (install from PyPI)
    - `tuna` (use Tsinghua University mirror)
    - `aliyun` (use Alibaba Cloud mirror)
    - `tencent` (use Tencent Cloud mirror)
    - `pip` (install via pip)
    - a custom PyPI mirror URL

    If not set, prek automatically selects the best available source.

- `PREK_NATIVE_TLS` - Use system's trusted store instead of the bundled `webpki-roots` crate.

- `PREK_CONTAINER_RUNTIME` - Specify the container runtime to use for container-based hooks (e.g., `docker`, `docker_image`). Options:

    - `auto` (default, auto-detect available runtime)
    - `docker`
    - `podman`
    - `container` (Apple's Container runtime on macOS, see [container](https://github.com/apple/container))

- `GITHUB_TOKEN` — GitHub personal access token for API requests. Used when downloading Bun toolchains to avoid GitHub API rate limits (60 requests/hour unauthenticated vs 5,000/hour authenticated). This is already set automatically in GitHub Actions.

Compatibility fallbacks:

- `PRE_COMMIT_ALLOW_NO_CONFIG` — Fallback for `PREK_ALLOW_NO_CONFIG`.
- `PRE_COMMIT_NO_CONCURRENCY` — Fallback for `PREK_NO_CONCURRENCY`.
- `SKIP` — Fallback for `PREK_SKIP`.
