# Common Workflows

This page summarizes the commands you normally use after a project already has a `prek.toml` or `.pre-commit-config.yaml`.

## Set Up Once

Install Git shims so `prek` runs automatically during Git operations:

```bash
prek install
```

If the repository used `pre-commit` before, overwrite the existing shims once:

```bash
prek install -f
```

Prepare hook environments ahead of time, which is useful for CI images or when you want the first commit to be fast:

```bash
prek install --prepare-hooks
```

## Run Hooks

Run hooks for the files currently staged in Git:

```bash
prek run
```

Run hooks against the whole repository, commonly after changing hook configuration or before opening a PR:

```bash
prek run --all-files
```

Run a single hook by ID:

```bash
prek run ruff
```

Run without changing files or executing hooks, to inspect what would run:

```bash
prek run --dry-run
```

## Inspect and Debug

List the hooks and projects discovered in the current workspace:

```bash
prek list
```

Validate configuration files:

```bash
prek validate-config prek.toml
```

Use `.pre-commit-config.yaml` instead if that is the repository's config file.

Inspect file type tags when `types`, `types_or`, or `exclude_types` filters do not match as expected:

```bash
prek util identify path/to/file
```

Use verbose output when a hook fails in a way that needs more context:

```bash
prek run -vvv
```

## Maintain Hooks

Update pinned hook repository revisions:

```bash
prek auto-update
```

Prepare hook environments without touching Git shims:

```bash
prek prepare-hooks
```

Show or clean cached repositories, hook environments, and toolchains:

```bash
prek cache dir
prek cache gc
prek cache clean
```

## Where to Go Next

- [Configuration](configuration.md) covers config file formats, discovery, and validation.
- [Workspace Mode](workspace.md) covers monorepos and nested project configs.
- [CLI Reference](reference/cli.md) lists every command and option.
