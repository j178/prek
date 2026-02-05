# Configuration

`prek` reads **one configuration file per project**. You only need to choose **one** format:

- **prek.toml** (TOML) — recommended for new users
- **.pre-commit-config.yaml** (YAML) — best if you already use pre-commit or rely on tool/editor support

Both formats are first-class and will be supported long-term. They describe the **same** configuration model: you list repositories under `repos`, then enable and configure hooks from those repositories.

=== "prek.toml"

    ```toml
    [[repos]]
    repo = "https://github.com/pre-commit/pre-commit-hooks"
    hooks = [{ id = "trailing-whitespace" }]
    ```

=== ".pre-commit-config.yaml"

    ```yaml
    repos:
      - repo: https://github.com/pre-commit/pre-commit-hooks
        hooks:
          - id: trailing-whitespace
    ```

## Pre-commit compatibility

`prek` is **fully compatible** with [`pre-commit`](https://pre-commit.com/) YAML configs, so your existing `.pre-commit-config.yaml` files work unchanged.

If you use **`prek.toml`**, there’s nothing to worry about from a `pre-commit` perspective: upstream `pre-commit` does not read TOML.

If you use the same `.pre-commit-config.yaml` with both tools, keep in mind:

- `prek` supports several extensions beyond upstream `pre-commit`.
- Upstream `pre-commit` may warn about unknown keys or error out on unsupported features.
- To stay maximally portable, avoid the extensions listed below (or keep separate configs).

Notable differences (when using YAML):

- **Workspace mode** is a `prek` feature that can discover multiple projects; upstream `pre-commit` is single-project.
- `files` / `exclude` can be written as **glob mappings** in `prek` (in addition to regex), which is not supported by upstream `pre-commit`.
- `repo: builtin` adds fast built-in hooks in `prek`.
- Upstream `pre-commit` uses `minimum_pre_commit_version`, while `prek` uses `minimum_prek_version` and intentionally ignores `minimum_pre_commit_version`.

### Prek-only extensions

These entries are implemented by `prek` and are not part of the documented upstream `pre-commit` configuration surface.
They work in both YAML and TOML, but they only matter for compatibility if you share a YAML config with upstream `pre-commit`.

- Top-level:
    - [`minimum_prek_version`](#prek-only-minimum-prek-version-config)
    - [`orphan`](#prek-only-orphan)
- Repo type:
    - [`repo: builtin`](#prek-only-repo-builtin)
- Hook-level:
    - [`env`](#prek-only-env)
    - [`priority`](#prek-only-priority)
    - [`minimum_prek_version`](#prek-only-minimum-prek-version-hook)

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

- `prek.toml` (TOML)
- `.pre-commit-config.yaml` (YAML, preferred for pre-commit compatibility)
- `.pre-commit-config.yml` (YAML, alternate)

In workspace mode, each project uses one of these filenames in its own directory.

!!! note "One format per repo"

    We recommend using a **single format** across the whole repository to avoid confusion.

    If multiple configuration files exist in the same directory, `prek` uses only one and ignores the rest.
    The precedence order is:

    1. `prek.toml`
    2. `.pre-commit-config.yaml`
    3. `.pre-commit-config.yml`

### File format

Both `prek.toml` and `.pre-commit-config.yaml` map to the same configuration model (repositories under `repos`, then `hooks` under each repo).

This section focuses on format-specific authoring notes and examples.

#### TOML (`prek.toml`)

Practical notes:

- Structure is explicit and less indentation-sensitive.
- Inline tables are common for hooks (e.g. `{ id = "ruff" }`).

TOML supports both **inline tables** and **array-of-tables**, so you can choose between a compact or expanded hook style.

Inline tables (best for small/simple hook configs):

```toml
[[repos]]
repo = "https://github.com/pre-commit/pre-commit-hooks"
rev = "v6.0.0"
hooks = [
  { id = "end-of-file-fixer", args = ["--fix"] },
]
```

Array-of-tables (more readable for larger hook configs):

```toml
[[repos]]
repo = "https://github.com/pre-commit/pre-commit-hooks"
rev = "v6.0.0"

[[repos.hooks]]
id = "trailing-whitespace"

[[repos.hooks]]
id = "check-json"
```

Example:

=== "prek.toml"

    ```toml
    default_language_version.python = "3.12"

    [[repos]]
    repo = "local"
    hooks = [
      {
        id = "ruff",
        name = "ruff",
        language = "system",
        entry = "python3 -m ruff check",
        files = "\\.py$",
      },
    ]
    ```

#### YAML (`.pre-commit-config.yaml` / `.yml`)

Practical notes:

- Regular expressions are provided as YAML strings.
  If your regex contains backslashes, quote it (e.g. `files: '\\.rs$'`).
- YAML anchors/aliases and merge keys are supported, so you can de-duplicate repeated blocks.

Example:

=== ".pre-commit-config.yaml"

    ```yaml
    default_language_version:
      python: "3.12"

    repos:
      - repo: local
        hooks:
          - id: ruff
            name: ruff
            language: system
            entry: python3 -m ruff check
            files: "\\.py$"
    ```

#### Choosing a format

**`prek.toml`**

- Clearer structure and less error-prone syntax.
- Recommended for new users or new projects.

**`.pre-commit-config.yaml`**

- Long-established in the ecosystem with broad tool/editor support.
- Fully compatible with upstream `pre-commit`.

**Recommendation**

- If you already use `.pre-commit-config.yaml`, keep it.
- If you want a cleaner, more robust authoring experience, prefer `prek.toml`.

If you want to switch, you have to do it manually for now; we might add `prek migrate` in the future to make it easy to convert YAML configs to `prek.toml`.

### Scope (per-project)

Each configuration file (`prek.toml`, `.pre-commit-config.yaml`, or `.pre-commit-config.yml`) is scoped to the **project directory it lives in**.

In workspace mode, `prek` treats every discovered configuration file as a **distinct project**:

- A project’s config only controls hook selection and filtering (for example `files` / `exclude`) for that project.
- A project may contain nested subprojects (subdirectories with their own config). Those subprojects run using *their own* configs.

Practical implication: filters in the parent project do not “turn off” a subproject.

Example layout (monorepo with a nested project):

- `foo/.pre-commit-config.yaml` (project `foo`)
- `foo/bar/.pre-commit-config.yaml` (project `foo/bar`, nested subproject)

If project `foo` config contains an `exclude` that matches `bar/**`, then hooks for project `foo` will not run on files under `foo/bar`:

=== "prek.toml"

    ```toml
    # foo/prek.toml
    exclude = { glob = "bar/**" }
    ```

=== ".pre-commit-config.yaml"

    ```yaml
    # foo/.pre-commit-config.yaml
    exclude:
      glob: "bar/**"
    ```

But if `foo/bar` is itself a project (has its own config), files under `foo/bar` are still eligible for hooks when running **in the context of project `foo/bar`**.

!!! note "Excluding a nested project"

    If `foo/bar/.pre-commit-config.yaml` exists but you *don’t* want it to be recognized as a project in workspace mode, exclude it from discovery using [`.prekignore`](workspace.md#discovery).

    Like `.gitignore`, `.prekignore` files can be placed anywhere in the workspace and apply to their directory and all subdirectories.

### Validation

Use `prek validate-config` to validate one or more config files.

If you want IDE completion / validation, prek provides a JSON Schema at [https://prek.j178.dev/docs/prek.schema.json](https://prek.j178.dev/docs/prek.schema.json).

And the schema is also submitted to the [JSON Schema Store](https://www.schemastore.org/prek.json), so some editors may pick it up automatically.

That schema tracks what `prek` accepts today, but `prek` also intentionally tolerates unknown keys for forward compatibility.

## Configuration reference

This section documents the configuration keys that `prek` understands.

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

!!! tip "Regex matching"

    When `files` / `exclude` are regex strings, they are matched with *search* semantics (the pattern can match anywhere in the path).
    Use `^` to anchor at the beginning and `$` at the end.

    `prek` uses the Rust [`fancy-regex`](https://github.com/fancy-regex/fancy-regex) engine.
    Most typical patterns are portable to upstream `pre-commit`, but very advanced regex features may differ from Python’s `re`.

!!! note "prek-only globs"

    In addition to regex strings, `prek` supports glob patterns via:

    - `files: { glob: "..." }` (single glob)
    - `files: { glob: ["...", "..."] }` (glob list)

    This is a `prek` extension. Upstream `pre-commit` expects regex strings here.

    For more information on the glob syntax, refer to the [globset documentation](https://docs.rs/globset/latest/globset/#syntax).

Examples:

=== "prek.toml"

    ```toml
    # Regex (portable to pre-commit)
    files = "\\.rs$"

    # Glob (prek-only)
    files = { glob = "src/**/*.rs" }

    # Glob list (prek-only; matches if any glob matches)
    files = { glob = ["src/**/*.rs", "crates/**/src/**/*.rs"] }
    ```

=== ".pre-commit-config.yaml"

    ```yaml
    # Regex (portable to pre-commit)
    files: "\\.rs$"

    # Glob (prek-only)
    files:
      glob: "src/**/*.rs"

    # Glob list (prek-only; matches if any glob matches)
    files:
      glob:
        - "src/**/*.rs"
        - "crates/**/src/**/*.rs"
    ```

<a id="top-level-exclude"></a>

#### `exclude`

Global *exclude* regex applied before hook-level filtering.

- Type: regex string (default, pre-commit compatible) **or** a prek-only glob pattern mapping
- Default: no global exclude filter

`exclude` is useful for generated folders, vendored code, or build outputs.

!!! note "prek-only globs"

    Like `files`, `exclude` supports `glob` (single glob or glob list) as a `prek` extension.
    For glob syntax details, see the [globset documentation](https://docs.rs/globset/latest/globset/#syntax).

Examples:

=== "prek.toml"

    ```toml
    # Regex (portable to pre-commit)
    exclude = "^target/"

    # Glob (prek-only)
    exclude = { glob = "target/**" }

    # Glob list (prek-only)
    exclude = { glob = ["target/**", "dist/**"] }
    ```

=== ".pre-commit-config.yaml"

    ```yaml
    # Regex (portable to pre-commit)
    exclude: "^target/"

    # Glob (prek-only)
    exclude:
      glob: "target/**"

    # Glob list (prek-only)
    exclude:
      glob:
        - "target/**"
        - "dist/**"
    ```

Verbose regex example (useful for long allow/deny lists):

=== "prek.toml"

    ```toml
    # `(?x)` enables "verbose" regex mode (whitespace and newlines are ignored).
    exclude = """(?x)^(
      docs/|
      vendor/|
      target/
    )"""
    ```

=== ".pre-commit-config.yaml"

    ```yaml
    # `(?x)` enables "verbose" regex mode (whitespace and newlines are ignored).
    exclude: |
      (?x)^(
        docs/|
        vendor/|
        target/
      )
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

=== "prek.toml"

    ```toml
    default_language_version.python = "3.12"
    default_language_version.node = "20"
    ```

=== ".pre-commit-config.yaml"

    ```yaml
    default_language_version:
      python: "3.12"
      node: "20"
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

=== "prek.toml"

    ```toml
    minimum_prek_version = "0.2.0"
    ```

=== ".pre-commit-config.yaml"

    ```yaml
    minimum_prek_version: "0.2.0"
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

=== "prek.toml"

    ```toml
    orphan = true

    [[repos]]
    repo = "https://github.com/astral-sh/ruff-pre-commit"
    rev = "v0.8.4"
    hooks = [{ id = "ruff" }]
    ```

=== ".pre-commit-config.yaml"

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

Remote hook definitions live inside the hook repository itself in the
`.pre-commit-hooks.yaml` manifest (at the repo root). Your config only selects
hooks by `id` and optionally overrides options. See [Authoring Hooks](authoring-hooks.md)
if you maintain a hook repository.

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

=== "prek.toml"

    ```toml
    [[repos]]
    repo = "https://github.com/astral-sh/ruff-pre-commit"
    rev = "v0.8.4"
    hooks = [{ id = "ruff", args = ["--fix"] }]
    ```

=== ".pre-commit-config.yaml"

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

=== "prek.toml"

    ```toml
    [[repos]]
    repo = "local"
    hooks = [
      {
        id = "cargo-fmt",
        name = "cargo fmt",
        language = "system",
        entry = "cargo fmt",
        files = "\\.rs$",
      },
    ]
    ```

=== ".pre-commit-config.yaml"

    ```yaml
    repos:
      - repo: local
        hooks:
          - id: cargo-fmt
            name: cargo fmt
            language: system
            entry: cargo fmt
            files: "\\.rs$"
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

=== "prek.toml"

    ```toml
    [[repos]]
    repo = "meta"
    hooks = [{ id = "check-useless-excludes" }]
    ```

=== ".pre-commit-config.yaml"

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

=== "prek.toml"

    ```toml
    [[repos]]
    repo = "builtin"
    hooks = [
      { id = "trailing-whitespace" },
      { id = "check-yaml" },
    ]
    ```

=== ".pre-commit-config.yaml"

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

=== "prek.toml"

    ```toml
    hooks = [{ id = "ruff", args = ["--fix"] }]
    ```

=== ".pre-commit-config.yaml"

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

=== "prek.toml"

    ```toml
    [[repos]]
    repo = "local"
    hooks = [
      {
        id = "cargo-doc",
        name = "cargo doc",
        language = "system",
        entry = "cargo doc --all-features --workspace --no-deps",
        env = { RUSTDOCFLAGS = "-Dwarnings" },
        pass_filenames = false,
      },
    ]
    ```

=== ".pre-commit-config.yaml"

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

See [Top-level `files`](#top-level-files) and [Top-level `exclude`](#top-level-exclude) for syntax notes and examples.

#### `types` / `types_or` / `exclude_types`

File-type filters based on [`identify`](https://pre-commit.com/#filtering-files-with-types) tags.

!!! tip

    Use [`prek identify <path>`](cli.md#prek-identify) to see how prek tags a file when you’re troubleshooting `types` filters.

Compared to regex-only filtering (`files` / `exclude`), tag-based filtering is often easier and more robust:

- tags can match by **file extension** *and* by **shebang** (for extensionless scripts)
- you can easily exclude things like **symlinks** or **binary files**

Common tags include:

- `file`, `text`, `binary`, `symlink`, `executable`

- language-ish tags such as `python`, `rust`, `javascript`, `yaml`, `toml`, ...

- `types`: all listed tags must match (logical AND)

- `types_or`: at least one listed tag must match (logical OR)

- `exclude_types`: tags that disqualify a file

How these combine:

- `files` / `exclude`, `types`, and `types_or` are combined with **AND**.
- Tags within `types` are combined with **AND**.
- Tags within `types_or` are combined with **OR**.

Defaults:

- `types`: `[file]` (matches all files)
- `types_or`: `[]`
- `exclude_types`: `[]`

These filters are applied in addition to regex filtering.

Examples:

=== "prek.toml"

    ```toml
    [[repos]]
    repo = "local"
    hooks = [
      # AND: must be under `src/` AND have the `python` tag
      {
        id = "lint-py",
        name = "Lint (py)",
        language = "system",
        entry = "python -m ruff check",
        files = "^src/",
        types = ["python"],
        exclude_types = ["symlink"]
      },

      # OR: match any of the listed tags under `web/`
      {
        id = "lint-web",
        name = "Lint (web)",
        language = "system",
        entry = "npm run lint",
        files = "^web/",
        types_or = ["javascript", "jsx", "ts", "tsx"]
      },
    ]
    ```

=== ".pre-commit-config.yaml"

    ```yaml
    repos:
      - repo: local
        hooks:
          - id: lint-py
            name: Lint (py)
            language: system
            entry: python -m ruff check
            files: ^src/
            types: [python]
            exclude_types: [symlink]

          - id: lint-web
            name: Lint (web)
            language: system
            entry: npm run lint
            files: ^web/
            types_or: [javascript, jsx, ts, tsx]
    ```

If you need to match a path pattern that doesn’t align with a hook’s default `types` (common when reusing an existing hook in a nonstandard way), override it back to “all files” and use `files`:

=== "prek.toml"

    ```toml
    [[repos]]
    repo = "meta"
    hooks = [
      {
        id = "check-hooks-apply",
        types = ["file"],
        files = "\\.(yaml|yml|myext)$"
      },
    ]
    ```

=== ".pre-commit-config.yaml"

    ```yaml
    repos:
      - repo: meta
        hooks:
          - id: check-hooks-apply
            types: [file]
            files: \.(yaml|yml|myext)$
    ```

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

=== "prek.toml"

    ```toml
    [[repos]]
    repo = "local"
    hooks = [
      {
        id = "format",
        name = "Format",
        language = "system",
        entry = "python3 -m ruff format",
        always_run = true,
        priority = 0,
      },
      {
        id = "lint",
        name = "Lint",
        language = "system",
        entry = "python3 -m ruff check",
        always_run = true,
        priority = 10,
      },
      {
        id = "tests",
        name = "Tests",
        language = "system",
        entry = "just test",
        always_run = true,
        priority = 20,
      },
    ]
    ```

=== ".pre-commit-config.yaml"

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

    === "prek.toml"

        ```toml
        hooks = [
          { id = "ruff", language = "python", language_version = "3.12" },
          { id = "eslint", language = "node", language_version = "20" },
          { id = "cargo-fmt", language = "rust", language_version = "stable" },
          { id = "my-tool", language = "system", language_version = "system" },
        ]
        ```

    === ".pre-commit-config.yaml"

        ```yaml
        hooks:
          - id: ruff
            language: python
            language_version: "3.12"

          - id: eslint
            language: node
            language_version: "20"

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

prek supports the following environment variables:

- `PREK_HOME` — Override the prek data directory (caches, toolchains, hook envs). If beginning with `~`, it is expanded to the user’s home directory. Defaults to `~/.cache/prek` on macOS and Linux, and `%LOCALAPPDATA%\prek` on Windows.

- `PREK_COLOR` — Control colored output: auto (default), always, or never.

- `PREK_QUIET` — Control quiet output mode. Set to `1` for quiet mode (equivalent to `-q`, only shows failed hooks), or `2` for silent mode (equivalent to `-qq`, no output to stdout).

- `PREK_SKIP` — Comma-separated list of hook IDs to skip (e.g. black,ruff). See [Skipping Projects or Hooks](workspace.md#skipping-projects-or-hooks) for details.

- `PREK_ALLOW_NO_CONFIG` — Allow running without a configuration file (useful for ad‑hoc runs).

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

- `PREK_NATIVE_TLS` — Use the system trusted store instead of the bundled `webpki-roots` crate.

- `PREK_CONTAINER_RUNTIME` — Specify the container runtime to use for container-based hooks (e.g., `docker`, `docker_image`). Options:

    - `auto` (default, auto-detect available runtime)
    - `docker`
    - `podman`
    - `container` (Apple's Container runtime on macOS, see [container](https://github.com/apple/container))

Compatibility fallbacks:

- `PRE_COMMIT_ALLOW_NO_CONFIG` — Fallback for `PREK_ALLOW_NO_CONFIG`.
- `PRE_COMMIT_NO_CONCURRENCY` — Fallback for `PREK_NO_CONCURRENCY`.
- `SKIP` — Fallback for `PREK_SKIP`.
