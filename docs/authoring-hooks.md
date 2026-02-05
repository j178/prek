# Authoring Hooks

This page is for hook authors who publish a repository consumed by end users.
If you only need to configure hooks in your own project, see [Configuration](configuration.md).

## Manifest file: `.pre-commit-hooks.yaml`

Hook repositories must include a `.pre-commit-hooks.yaml` file at the repo root.
The manifest is a YAML list of hook definitions. Each hook entry must include:

- `id`: stable identifier used in end-user configs
- `name`: human-friendly label shown in output
- `entry`: command to execute
- `language`: execution environment (for example `python`, `node`, `system`)

Hooks should exit non-zero on failure (or modify files and exit non-zero for fixers).

Common optional fields include `args`, `files`, `exclude`, `types`, `types_or`,
`stages`, `pass_filenames`, `description`, `additional_dependencies`, and
`require_serial`.

`prek` follows the upstream pre-commit manifest format. For the full field list
and semantics, see: https://pre-commit.com/#new-hooks

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

## Choosing hook stages

Hook authors can declare which git hook stages they support with `stages` in
`.pre-commit-hooks.yaml`. End users can override that list in their
configuration. If neither is set, `prek` falls back to the top-level
`default_stages` (which defaults to all stages).

The `manual` stage is special: it never runs automatically and is only executed
when a user explicitly runs `prek run --hook-stage manual <hook-id>`.

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

## Versioning for `prek auto-update`

End users pin your repository using the `rev` field in their config. To make
[`prek auto-update`](cli.md#prek-auto-update) work as expected, publish git tags for releases:

- Prefer semantic version tags like `v1.2.3` or `1.2.3`.
- Push tags to the remote (annotated or lightweight tags both work).
- Avoid moving tags; treat them as immutable release references.

`prek auto-update` selects the newest tag by default. With `--bleeding-edge`, it
uses the default branch tip instead of tags. With `--freeze`, it writes commit
SHAs into `rev` instead of tag names.

## Develop locally with `prek try-repo`

[`prek try-repo`](cli.md#prek-try-repo) runs hooks from a repository without publishing a release. This
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

Validate your manifest locally with [`prek validate-manifest`](cli.md#prek-validate-manifest):

```bash
prek validate-manifest .pre-commit-hooks.yaml
```

This ensures the manifest is well-formed before publishing a release tag.
