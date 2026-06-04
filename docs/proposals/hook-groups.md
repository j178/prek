# Hook groups for run selection

This document outlines the design for selecting hooks by user-defined hook
groups.

## Motivation

`prek run --all-files` already works in CI, agent workflows, and other
explicit command-line contexts. However, users sometimes need to run only a
project-specific subset of hooks:

- CI should run lint and formatting hooks, but skip tests that are covered by a
  separate job.
- CI should split checks and formatters into separate jobs.
- Agents should run slow type checkers before submitting work, while human
  contributors should not be blocked by those hooks on every commit.
- Some hooks should be opt-in locally, but enabled for a specific merge-request
  or release workflow.
- Some hooks should be excluded from a local run because they require large
  local toolchains or ecosystems the user intentionally avoids.

Today, users can approximate this with hook `stages`, for example by adding
`manual` to hooks and running `prek run --stage manual`. That is confusing:
`stages` describe Git hook contexts, while these use cases describe arbitrary
execution profiles. Adding dedicated stages such as `ci`, `docker`, `release`,
or `agent` would repeat the same problem and require a growing vocabulary.

The goal is to add a small hook-level tagging mechanism that lets users define
their own run profiles without changing Git hook stage semantics.

## Configuration

### Hook Configuration: `groups`

A new optional field `groups` is added to project hook configuration.

```yaml
repos:
  - repo: local
    hooks:
      - id: format
        name: Format Python
        entry: ruff format
        language: system
        groups: ["format", "ci"]

      - id: lint
        name: Lint Python
        entry: ruff check
        language: system
        groups: ["lint", "ci"]

      - id: typecheck
        name: Typecheck Python
        entry: pyright
        language: system
        groups: ["slow", "agent"]
```

- **Type**: list of strings
- **Default**: empty list
- **Scope**: project configuration only
- **Matching**: exact and case-sensitive
- **Names**: non-empty strings without whitespace

`groups` is a `prek`-only field. It is not part of upstream `pre-commit`, and it
should not be treated as remote hook manifest metadata. Groups describe how the
current project wants to run hooks, not an intrinsic property of a hook
repository. If a remote hook manifest contains `groups`, `prek` should warn and
ignore the field, the same way it treats other config-only hook fields such as
`priority`.

For `prek.toml`, use the same field name:

```toml
[[repos]]
repo = "local"
hooks = [
  {
    id = "format",
    name = "Format Python",
    entry = "ruff format",
    language = "system",
    groups = ["format", "ci"],
  },
]
```

## CLI

Add two repeatable options to `prek run`:

```text
prek run --group <name>
prek run --no-group <name>
```

`--group <name>` is an include filter. When one or more groups are requested, a
hook is selected if its `groups` contains at least one requested group.

`--no-group <name>` is an exclude filter. A hook is removed if its `groups`
contains any excluded group.

If both options are provided, exclusion wins:

1. Select hooks matching `--group`, if any `--group` values were provided.
2. Remove hooks matching `--no-group`, if any `--no-group` values were provided.

Examples:

```bash
prek run --all-files --group ci
prek run --all-files --group lint --group typecheck
prek run --all-files --no-group format
prek run --all-files --group ci --no-group slow
prek run --all-files --group ci --stage pre-push
```

## Selection Model

Group filtering composes with existing project and hook selectors. The effective
selection order should be:

1. Load hooks from selected projects.
2. Apply positional hook or project includes.
3. Apply `--skip` selectors and skip environment variables.
4. Apply group include and exclude filters.
5. Apply explicit `--stage` filtering, if provided.
6. If no group filter and no explicit `--stage` were provided, apply the
   existing default `pre-commit` stage filtering and hook-target `manual`
   fallback.
7. Apply the existing file matching and run-input logic.

For example:

```bash
prek run frontend/ --group ci --no-group slow
```

This runs hooks in `frontend/` that are tagged with `ci`, except hooks also
tagged with `slow`.

Hooks excluded by group filtering must not be installed or executed for that
run. This matters for hooks that require large toolchains, unsupported local
dependencies, or ecosystems the user intentionally does not want to invoke.

## Stage Interaction

Groups are independent from Git hook stages.

When `--group` and `--no-group` are not used, the existing stage behavior is
unchanged: omitting `--stage`/`--hook-stage` first selects hooks eligible for
`pre-commit`. If no hook is selected and the command named hook IDs, those same
IDs are matched again against hooks configured for `manual`.

When `--group` or `--no-group` is used without an explicit `--stage`, `prek run`
enters group selection mode:

- Hooks from any configured stage can match.
- The special second pass that checks named hook IDs against `manual` is not
  used.
- File input is collected with the normal manual `prek run` file mode: explicit
  `--files`/`--directory`, `--all-files`, merge conflicts, or staged files.
- Hooks configured only for `commit-msg` and/or `prepare-commit-msg` cannot run
  in this mode because they require Git's message file argument, so they are
  filtered out.

This works because manual `prek run --group ...` has no Git hook payload. All
non-message-file stages can be executed with file input or no filenames
according to each hook's normal filters and `pass_filenames` setting; only the
message-file stages need input that cannot be inferred and are not selected by
stage-less group runs.

When `--group` or `--no-group` is combined with explicit stage selection, the
filters compose by intersection:

```bash
prek run --group ci --stage pre-push
```

This runs hooks that are both tagged `ci` and eligible for `pre-push`.
Equivalent `--hook-stage` spelling should behave the same way.

This keeps group-based CI usage simple because `prek run --group ci` does not
require users to add `manual` to every CI hook. It also lets users narrow a
group to a real Git hook context when that is what they intend.

## Runtime Semantics

Groups only decide which hooks are eligible to run. After that, existing hook
behavior remains unchanged:

- `files`, `exclude`, `types`, `types_or`, and `exclude_types` still filter
  files.
- `always_run` only applies after a hook survives group filtering.
- `pass_filenames: false` remains a hook execution setting, not a group
  selection setting.
- `priority` continues to schedule the remaining hooks.
- `fail_fast`, `require_serial`, diff detection, modified-file reporting,
  output handling, and hook result semantics are unchanged.
- Language support checks still apply to selected hooks.
- Existing hook cache and install behavior should only consider hooks that
  survived group filtering.

If group filtering leaves no hooks to run, `prek run` should report that no
hooks matched the requested selectors and return failure, matching the behavior
for explicit selector mistakes.

## Workspace Behavior

In workspace mode, group names are CLI selectors applied across all selected
projects:

- `prek run --group ci` selects every hook tagged `ci` in every selected
  project.
- Project selectors can narrow the workspace scope before group filtering.
- Group names do not need to be declared globally.
- Group names do not coordinate scheduling across project config files.

Group matching is evaluated per hook. A project with no matching hooks is simply
excluded from the run.

## Edge Cases

### Ungrouped Hooks

Hooks with omitted `groups` or `groups: []` are ungrouped.

If no group options are passed, ungrouped hooks run as they do today.

If `--group <name>` is passed, ungrouped hooks do not match and are not run.
This proposal does not add a virtual `ungrouped` group.

If only `--no-group <name>` is passed, ungrouped hooks remain selected, because
they do not belong to the excluded group.

### Multiple Groups

A hook may belong to multiple groups:

```yaml
- id: ruff
  groups: ["lint", "python", "ci"]
```

The hook matches any include group and is excluded by any exclude group:

- `--group lint` selects it.
- `--group ci --no-group python` excludes it.
- `--no-group format` does not exclude it.

### Duplicate Group Names

Duplicate group names on the same hook should be treated as a single group
membership. Implementations may deduplicate during parsing or matching.

```yaml
groups: ["ci", "ci"]
```

### Invalid Group Names

Group names should be non-empty strings and must not contain whitespace. Names
are matched exactly, so implementations should not trim or normalize group
names before validation.

Invalid examples:

```yaml
groups: ["", "ci slow", " ci", "ci\nslow"]
```

No fixed vocabulary should be enforced. In particular, `ci`, `agent`, `slow`,
`format`, and `lint` are examples, not reserved names.

### Case Sensitivity

Group matching should be case-sensitive. `CI` and `ci` are distinct groups.

This avoids platform-specific normalization surprises and matches most existing
configuration key/value behavior.

### Unknown CLI Groups

If `--group does-not-exist` matches no hooks, the run should fail with the same
kind of explicit-selection error used for unmatched hook selectors.

If multiple groups are requested and at least one matches, unmatched group names
should produce a warning rather than failing the entire run, consistent with
the existing selector reporting style.

For example:

```bash
prek run --group ci --group does-not-exist
```

should run `ci` hooks and warn that `does-not-exist` matched no hooks.

### Skip Selectors

`--skip` continues to remove hooks even if they match a group:

```bash
prek run --group ci --skip ruff
```

The `ruff` hook is skipped.

### Environment Skip Variables

Existing skip environment variables continue to apply. They should not gain a
new group syntax in this proposal.

Adding environment-level group selection such as `PREK_GROUP` or
`PREK_NO_GROUP` can be discussed separately if needed.

### Local and Remote Hooks

Groups apply equally to local, remote, meta, and builtin hooks when the project
configuration can attach hook options to them.

Remote hook manifests should not define default groups. A remote repository
does not know the consuming project's CI, agent, or local workflow policy. If a
manifest contains `groups`, the field is ignored with a warning.

### Priority Scheduling

`groups` are selectors. They are not scheduler groups.

After group filtering, the remaining hooks are scheduled by existing `priority`
semantics. Hooks with the same `priority` may still run in parallel. Hooks in
the same `groups` value do not gain any ordering or concurrency relationship.

### Modified Files

Group selection does not change modified-file detection. If selected hooks
modify files, existing failure and diff reporting behavior applies.

If a hook is excluded by group filtering, file modifications that hook would
have made obviously cannot occur, and `prek` should not install or execute it.

### Try Repo

`prek try-repo` embeds `RunArgs`, so it can accept `--group` and `--no-group`.
The options should work when the temporary hook configuration includes
`groups`.

If the temporary configuration has no groups, `--group <name>` should match no
hooks.

### List Command

This proposal does not require changes to `prek list`.

It would be useful for `prek list --output-format=json` to include `groups` once
the field exists, but filtering `prek list` by group can be a follow-up.

## Install Behavior

This proposal should not add `prek install --group`.

Persisting group choices into installed Git shims would make it hard to inspect
what actually runs from a normal Git hook. Installed hooks should keep using
stage semantics.

Users who want profile-based execution can invoke `prek run --group ...`
explicitly in CI, agent workflows, or contributor documentation.

Future proposals may discuss install-time groups or default groups, but that
changes the meaning of what runs by default and should be designed separately.

## Non-goals

This proposal does not add:

- A new `ci` stage.
- A scheduler or dependency group. `priority` remains the scheduling mechanism.
- DAG scheduling or `after` dependencies.
- `--output-format=grouped`.
- A virtual `ungrouped` selector.
- Default-disabled hooks.
- Environment variables for group selection.
- `prek install --group`.
- Global group declarations or validation against a root-level list.

## Backward Compatibility

If no `groups`, `--group`, or `--no-group` is used, behavior is unchanged.

Existing `stages` behavior is unchanged for normal `prek run` and installed Git
hook execution. The only new behavior is explicit group selection mode and the
ability to intersect that mode with an explicitly requested stage.

Existing configs that contain an unknown `groups` key currently ignore it. Once
this proposal is implemented, that key becomes meaningful. This is acceptable
because it is an additive feature and unknown keys are not part of a guaranteed
stable behavior contract.

## Implementation Notes

At a high level, implementation should:

1. Add `groups` to config hook types and the built `Hook` type.
2. Validate group names during config parsing or hook construction.
3. Add repeatable `--group` and `--no-group` arguments to `prek run`.
4. Apply group filtering before explicit stage filtering and before install
   selection.
5. Skip default stage filtering when group mode is active and no explicit
   `--stage` was provided.
6. Add schema, documentation, and CLI reference updates.
7. Add integration tests covering include, exclude, include-plus-exclude,
   ungrouped hooks, explicit stage intersections, workspace selection, and
   install avoidance for excluded hooks.
