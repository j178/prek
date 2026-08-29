# Authoring Hooks

This page is for hook authors who publish a repository consumed by end users.
If you only need to configure hooks in your own project, see
[Run Existing Project Commands](local-hooks.md).

A minimal hook repository has a manifest at its root plus the source and
packaging files required by its language. For example, a Python hook might use:

```text
my-hook/
├── .pre-commit-hooks.yaml
├── pyproject.toml
└── src/
    └── my_hook/
        └── __init__.py
```

The exact packaging files vary by language. The manifest tells consumers which
installed command to run; the language backend determines how the repository is
installed.

## Manifest file: `.pre-commit-hooks.yaml`

Hook repositories must include a `.pre-commit-hooks.yaml` file at the repo root.
There is no separate `prek` manifest format; `prek` reads the same
`.pre-commit-hooks.yaml` manifest defined by upstream `pre-commit`. This keeps
hook repositories compatible with the broader pre-commit ecosystem.

Hooks should exit non-zero on failure (or modify files and exit non-zero for fixers).

The manifest is a YAML list of hook definitions. `prek` supports these fields in
each manifest hook:

| Field | Required | `prek`-only | Type | Description |
| -- | -- | -- | -- | -- |
| `id` | Yes | No | string | Stable identifier used in end-user configs. |
| `name` | Yes | No | string | Human-friendly label shown in output. |
| `entry` | Yes | No | string | Command to execute. |
| `shell` | No | Yes | string enum | Run `entry` through a predefined shell adapter (`sh`, `bash`, `pwsh`, `powershell`, or `cmd`). |
| `language` | Yes | No | string | Execution environment, for example `python`, `node`, or `system`. |
| `alias` | No | No | string | Alternate identifier accepted by `prek run`. |
| `files` | No | No | regex string or glob map | Include only matching files. |
| `exclude` | No | No | regex string or glob map | Exclude matching files. |
| `types` | No | No | list of strings | Require all listed file type tags. |
| `types_or` | No | No | list of strings | Require at least one listed file type tag. |
| `exclude_types` | No | No | list of strings | Exclude files with any listed file type tag. |
| `additional_dependencies` | No | No | list of strings | Extra dependencies installed into managed hook environments. |
| `args` | No | No | list of strings | Extra arguments appended to `entry` before filenames. |
| `env` | No | Yes | map of strings | Environment variables for hook environment creation and execution. |
| `always_run` | No | No | boolean | Run even when no files match. |
| `fail_fast` | No | No | boolean | Stop the run immediately if this hook fails. |
| `pass_filenames` | No | No | boolean or positive integer | Control whether, or how many, matching filenames are passed. |
| `description` | No | No | string | Free-form metadata shown in listings; its first line is also shown with run details. |
| `language_version` | No | No | string or map | Language/toolchain version request and source preference. |
| `log_file` | No | No | string path | Write hook output to a file when the hook fails or is verbose. |
| `require_serial` | No | No | boolean | Avoid concurrent invocations of this hook. |
| `stages` | No | No | list of stage names | Git hook stages where this hook is eligible to run. |
| `verbose` | No | No | boolean | Print output even when the hook succeeds. |
| `minimum_prek_version` | No | Yes | version string | Minimum `prek` version required for this hook. |

For fields shared with upstream `pre-commit`, `prek` follows the upstream
manifest semantics. For the upstream reference, see:
[https://pre-commit.com/#new-hooks](https://pre-commit.com/#new-hooks).

!!! note "`prek`-only manifest fields"

    `prek`-only fields are accepted by `prek`, but upstream `pre-commit` will not
    recognize them.

    End-user configuration may also set [`env`](reference/configuration.md#prek-only-env)
    and [`shell`](reference/configuration.md#shell). When both the manifest and end-user
    config define `env`, the maps are merged and end-user values override
    duplicate keys.

    `pass_filenames: n` with a positive integer is also a `prek` extension.
    Upstream `pre-commit` only accepts a boolean value.

    The `{ glob: ... }` mapping form for `files` and `exclude` is a `prek`
    extension. Use the regex string form when a manifest must also work with
    upstream `pre-commit`.

    The `language_version` options map with `request` and `preference` fields is
    a `prek` extension. Use the string form when a manifest must also work with
    upstream `pre-commit`.

    When `shell` is set, `entry` is treated as shell source. Hook `args` and
    filenames are passed as script arguments, so POSIX shell entries should read
    them with `"$@"`. `shell` is supported only for language backends that use
    the shell-aware entry resolver; see [`shell`](reference/configuration.md#shell) for
    the supported languages and exact shell adapter commands.

!!! note "Manifest fields only"

    Project configuration-only fields, such as `priority` and `groups`, are not
    manifest hook fields.

### Editor completion and validation

`prek` maintains a [`prek-hooks.schema.json`](https://raw.githubusercontent.com/j178/prek/master/prek-hooks.schema.json)
schema for `.pre-commit-hooks.yaml`. The schema stays in the `prek` repository
instead of being registered with SchemaStore, so editors must opt into it
explicitly.

With YAML Language Server, add this directive at the top of the manifest:

```yaml
# yaml-language-server: $schema=https://raw.githubusercontent.com/j178/prek/master/prek-hooks.schema.json
```

This enables completion and validation for both upstream fields and `prek`
extensions such as glob filters, `env`, and `shell`.

Example:

```yaml
- id: format-json
  name: format json
  entry: python3 -m tools.format_json
  language: python
  files: "\\.json$"

- id: lint-shell
  name: shellcheck
  entry: shellcheck
  language: system
  types: [shell]
```

Prefer entries that invoke an executable directly. Do not assume a shell is
present or that POSIX paths work on Windows unless the hook explicitly declares
that platform requirement. The [Language Support](languages.md) and
[Hook Entry Resolution](internals.md#hook-entry-resolution) pages describe the
runtime and working-directory contracts.

## Choosing hook stages

Hook authors can declare which Git hook stages they support with `stages` in
`.pre-commit-hooks.yaml`. End users can override that list in their
configuration. If neither is set, `prek` falls back to the top-level
`default_stages` (which defaults to all stages).

The `manual` stage is special: it never runs automatically and is only executed
when a user explicitly runs `prek run --hook-stage manual <hook-id>`.

For what each stage means and whether it operates on repository files, see
[Supported Git Hook Stages](reference/configuration.md#supported-git-hook-stages).

Example:

```yaml
- id: lint
  name: lint
  entry: my-lint
  language: python
  stages: [pre-commit, pre-merge-commit, pre-push, manual]
```

## Passing arguments to hooks

When users configure a hook with `args`, `prek` passes those arguments before
the list of file paths. If `args` is empty or omitted, only file paths are
provided.

Example end-user config:

```yaml
repos:
  - repo: https://github.com/example/hook-repo
    rev: v1.0.0
    hooks:
      - id: my-hook
        args: [--max-line-length=120]
```

Invocation shape:

```text
my-hook --max-line-length=120 path/to/file1 path/to/file2
```

Hook processes also receive stage-specific `PRE_COMMIT_*` variables. See
[Variables exposed to hooks](reference/environment-variables.md#variables-exposed-to-hooks)
for the values available during `pre-push`, commit-message, rebase, checkout,
and rewrite stages.

## Versioning for `prek update`

End users pin your repository using the `rev` field in their config. To make
[`prek update`](reference/cli.md#prek-update) work as expected, publish git tags for releases:

- Prefer semantic version tags like `v1.2.3` or `1.2.3`.
- Push tags to the remote (annotated or lightweight tags both work).
- Avoid moving tags; treat them as immutable release references.

`prek update` selects the newest tag by default. With `--bleeding-edge`, it
uses the default branch tip instead of tags. With `--freeze`, it writes commit
SHAs into `rev` instead of tag names.

## Develop locally with `prek try-repo`

[`prek try-repo`](reference/cli.md#prek-try-repo) runs hooks from a repository without publishing a release. This
is handy while iterating on a hook.

```bash
# In another repository where you want to test the hook
prek try-repo ../path/to/hook-repo my-hook-id --verbose
```

Notes:

- `prek try-repo` accepts any path or git URL `git clone` understands.
- For `prepare-commit-msg` or `commit-msg` hooks, pass the appropriate
  `--commit-msg-filename` argument when testing.

## Validation and CI

Validate your manifest locally with [`prek validate-manifest`](reference/cli.md#prek-validate-manifest):

```bash
prek validate-manifest .pre-commit-hooks.yaml
```

This ensures the manifest is well-formed before publishing a release tag.

Run that command in CI, then exercise the hook against a small fixture
repository or with `prek try-repo`. See [Continuous Integration](ci.md) for the
general CI setup.
