# Configuration

Use a project config to choose hooks and customize their options. If you need a
starter config, follow the [Quickstart](quickstart.md). This page covers editing,
validating, and updating that config; [Running Hooks](running-hooks.md) covers
executing it.

## Choose a config file

prek reads one config per project. Keep an existing `.pre-commit-config.yaml`, or use
`prek.toml` for a new setup. Both formats describe the same configuration model.

| Filename | Format |
| -- | -- |
| `prek.toml` | TOML |
| `.pre-commit-config.yaml` | YAML |
| `.pre-commit-config.yml` | YAML |

If several of these files exist in one directory, prek uses the first one in
the order above. Use one file per project to make it clear which config applies.

To convert a YAML config, run
[`prek util yaml-to-toml`](reference/cli.md#prek-util-yaml-to-toml). The conversion
does not preserve YAML comments. If the config must also work with upstream
pre-commit, keep YAML and read [Sharing a config with pre-commit](#sharing-a-config-with-pre-commit).

## Add and configure hooks

List hook repositories under `repos`, pin each remote repository to a `rev`,
and select its hooks by `id`:

=== "prek.toml"

    ```toml
    [[repos]]
    repo = "https://github.com/pre-commit/pre-commit-hooks"
    rev = "v6.0.0"
    hooks = [
      { id = "trailing-whitespace" },
      { id = "check-added-large-files", args = ["--maxkb=1024"] },
    ]
    ```

=== ".pre-commit-config.yaml"

    ```yaml
    repos:
      - repo: https://github.com/pre-commit/pre-commit-hooks
        rev: v6.0.0
        hooks:
          - id: trailing-whitespace
          - id: check-added-large-files
            args: [--maxkb=1024]
    ```

The hook repository supplies defaults such as the command and language. Add
options to a hook entry to customize it. Here, `args` changes the large-file
limit to 1024 KB. Consult the hook's documentation for the arguments it accepts.

You can also use:

- [Local Hooks](local-hooks.md) to define commands directly in the project config.
- [Built-in Hooks](built-in-hooks.md) to use hooks bundled with prek.

### TOML and YAML syntax

For larger TOML hook entries, use an array of tables instead of an inline table.
This is equivalent to the large-file hook above:

```toml
[[repos]]
repo = "https://github.com/pre-commit/pre-commit-hooks"
rev = "v6.0.0"

[[repos.hooks]]
id = "check-added-large-files"
args = ["--maxkb=1024"]
```

prek also accepts multiline inline tables from TOML 1.1. If an editor does not
support that syntax, use the array-of-tables form above.

In YAML, quote regular expressions containing backslashes, for example
`files: '\.rs$'`. YAML anchors, aliases, and merge keys can reuse repeated
configuration.

## Choose which files and stages to check

A hook's `files`, `exclude`, `types`, `types_or`, and `exclude_types` options
control which files it receives. Top-level `files` and `exclude` apply to every
hook in that project. For example, exclude generated files from all hooks by
adding this at the top of the config, before the repository entries:

=== "prek.toml"

    ```toml
    exclude = '^generated/'
    ```

=== ".pre-commit-config.yaml"

    ```yaml
    exclude: '^generated/'
    ```

`files` and `exclude` accept regular expressions or prek's explicit glob form.
Type filters use file type tags; inspect a file's tags with:

```bash
prek util identify path/to/file
```

A hook's `stages` limits the Git hook stages where it runs. To install the
corresponding Git shim, use `default_install_hook_types` or
[`prek install --hook-type`](reference/cli.md#prek-install--hook-type).
Setting `stages` alone does not install a shim.

See the [Configuration Reference](reference/configuration.md) for filter
combinations, stage names, and all available options.

## Config location and scope

prek searches upward from the current directory for a config, stopping at the
Git repository root. The first config found defines the workspace root; prek
then discovers nested projects below it.

Each project applies its config independently. A parent's filters do not
disable a child's hooks, and a child config does not override its parent.
See [Monorepos](monorepos.md) for setting up and selecting nested projects, or
the [Workspace Reference](reference/workspace.md) for discovery and file-scope
rules.

Passing `--config` selects one config and disables workspace discovery. Hooks
then run from the Git repository root with
[repository-relative paths](reference/workspace.md#single-config-mode).

## Validate changes

After editing a config, validate it and run the hooks against existing files:

```bash
prek validate-config prek.toml
prek run --all-files
```

Use the repository's YAML config filename instead if applicable.
[`prek validate-config`](reference/cli.md#prek-validate-config) accepts one or
more config files. See
[Debugging](debugging.md#a-hook-is-skipped-or-receives-no-files) if a hook is
skipped unexpectedly.

If you want IDE completion / validation, prek publishes a JSON Schema through the [JSON Schema Store](https://www.schemastore.org/prek.json), so some editors may pick it up automatically.

## Update hook versions

Update pinned remote hook revisions with:

```bash
prek update
```

Review the config diff, then run `prek run --all-files` to check the updated
hooks against the repository. Use `prek update --check` to check for available
updates without changing the config. See [`prek update`](reference/cli.md#prek-update)
for selecting repositories and controlling updates.

## User settings

`prek` also reads an optional user-level global config from the platform config directory:

- Linux and macOS: `~/.config/prek/prek.toml` (or `$XDG_CONFIG_HOME/prek/prek.toml` when `XDG_CONFIG_HOME` is set)
- Windows: `%APPDATA%\prek\prek.toml`

This file is for user-level prek settings, not hook definitions. Project hooks
still live in the project config files described above. For the supported
global settings, see the
[configuration reference](reference/configuration.md#global-config-file).

## Sharing a config with pre-commit

Existing pre-commit YAML configs work in prek. To use the same config with both
tools, keep the YAML format and avoid prek-only extensions. Upstream pre-commit
may warn about unknown keys or reject unsupported features.

See [Compatibility](compatibility.md#if-you-need-strict-upstream-portability)
for the features that affect portability and [Differences](diff.md) for broader
behavior differences.
