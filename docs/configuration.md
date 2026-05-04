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

## Global configuration

`prek` also reads an optional user-level global config from the platform config directory:

- Linux and macOS: `~/.config/prek/prek.toml` (or `$XDG_CONFIG_HOME/prek/prek.toml` when `XDG_CONFIG_HOME` is set)
- Windows: `%APPDATA%\prek\prek.toml`

This file is for user-level `prek` settings, not hook definitions. Project hooks still live in the project config files described below.

The first supported global setting is the default cooldown for `prek auto-update`:

```toml
[auto_update]
cooldown_days = 7
```

`prek auto-update --cooldown-days <DAYS>` overrides this value for a single command invocation.

## Pre-commit compatibility

`prek` is **fully compatible** with [`pre-commit`](https://pre-commit.com/) YAML configs, so your existing `.pre-commit-config.yaml` files work unchanged.

If you use **`prek.toml`**, there’s nothing to worry about from a `pre-commit` perspective: upstream `pre-commit` does not read TOML.

If you use the same `.pre-commit-config.yaml` with both tools, avoid `prek`-only extensions or keep separate configs.
Upstream `pre-commit` may warn about unknown keys or error out on unsupported features.
For broader behavior differences, see [Compatibility](compatibility.md) and [Differences](diff.md).

### Prek-only extensions

These entries are implemented by `prek` and are not part of the documented upstream `pre-commit` configuration surface.
They work in both YAML and TOML, but they only matter for compatibility if you share a YAML config with upstream `pre-commit`.

- Top-level:
    - [`minimum_prek_version`](reference/configuration.md#prek-only-minimum-prek-version-config)
    - [`orphan`](reference/configuration.md#prek-only-orphan)
- Repo type:
    - [`repo: builtin`](reference/configuration.md#prek-only-repo-builtin)
- Hook-level:
    - [`env`](reference/configuration.md#prek-only-env)
    - [`shell`](reference/configuration.md#shell)
    - [`priority`](reference/configuration.md#prek-only-priority)
    - [`minimum_prek_version`](reference/configuration.md#prek-only-minimum-prek-version-hook)

## Configuration file

### Location (discovery)

By default, `prek` looks for a configuration file starting from your current working directory and moving upward.
It stops when it finds a config file, or when it hits the git repository boundary.

If you run **without** `--config`, `prek` then enables **workspace mode**:

- The first config found while traversing upward becomes the workspace root.
- From that root, `prek` searches for additional config files in subdirectories (nested projects).

Workspace discovery respects `.gitignore`, and also supports `.prekignore` for excluding directories from discovery.
For the full behavior and examples, see [Workspace Mode](workspace.md).

!!! tip

    After updating `.prekignore`, run with `--refresh` to force a fresh project discovery so the changes are picked up.

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

The previous example uses multiline inline tables, a feature that was introduced in
[TOML 1.1](https://toml.io/en/v1.1.0), not all parsers have support for it yet.
You may want to use the longer form if your editor/IDE complains about it.

=== "prek.toml"

    ```toml
    default_language_version.python = "3.12"

    [[repos]]
    repo = "local"

    [[repos.hooks]]
    id = "ruff"
    name = "ruff"
    language = "system"
    entry = "python3 -m ruff check"
    files = "\\.py$"
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

!!! tip

    If you want to switch, you can use [`prek util yaml-to-toml`](reference/cli.md#prek-util-yaml-to-toml) to convert YAML configs to `prek.toml`.
    YAML comments are not preserved during conversion.

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

!!! tip

    After updating `.prekignore`, run with `--refresh` to force a fresh project discovery so the changes are picked up.

### Validation

Use [`prek validate-config`](reference/cli.md#prek-validate-config) to validate one or more config files.

If you want IDE completion / validation, prek publishes a JSON Schema through the [JSON Schema Store](https://www.schemastore.org/prek.json), so some editors may pick it up automatically.

That schema tracks what `prek` accepts today, but `prek` also intentionally tolerates unknown keys for forward compatibility.

For every accepted configuration key and hook option, see the [Configuration Reference](reference/configuration.md). For process environment controls, see the [Environment Variable Reference](reference/environment-variables.md).
