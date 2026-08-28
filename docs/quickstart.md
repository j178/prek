# Quickstart

This page helps you get productive with **prek** in minutes, whether you are migrating from [pre-commit](https://pre-commit.com/) or starting from scratch.

First follow the [installation guide](./installation.md) to install prek on your system.

[I already use pre-commit](#already-using-pre-commit){ .md-button .md-button--primary }
[I'm new to pre-commit-style tools](#new-to-pre-commit-style-workflows){ .md-button }
{: style="display:flex; flex-wrap:wrap; gap:1rem; justify-content:center; margin:1.5rem 0;" }

## Already using pre-commit?

Great news - prek is designed as a drop-in replacement, you only need two tweaks:

1. Replace every `pre-commit` command in your scripts or documentation with `prek`. Your existing `.pre-commit-config.yaml` continues to work unchanged.

    ```console
    $ prek run
    trim trailing whitespace.................................................Passed
    fix end of files.........................................................Passed
    typos....................................................................Passed
    cargo fmt................................................................Passed
    cargo clippy.............................................................Passed
    ```

2. Reinstall the Git shims once via `prek install -f` (run this if you previously executed `pre-commit install`).

From here you can explore what prek adds on top of pre-commit:

- [Key differences and new features](./diff.md)
- [Built-in Rust-native hooks](./builtin.md)
- [Workspace mode for monorepos](./workspace.md)

## New to pre-commit-style workflows?

Follow this short example to experience how prek automates linting and formatting tasks.

### 1. Initialize the repository

Run `prek init` from anywhere in your Git worktree:

```bash
prek init
```

This creates a starter `prek.toml` at the Git worktree root and installs the
`pre-commit` Git shim. If the root already has a supported configuration file,
prek keeps it unchanged and installs the shim.

The generated configuration uses prek's built-in hooks:

```toml
[[repos]]
repo = "builtin"
hooks = [
  { id = "trailing-whitespace" },
  { id = "end-of-file-fixer" },
  { id = "check-added-large-files" },
]
```

!!! note

    `prek.toml` is the native configuration file for **prek**. If you already have a `.pre-commit-config.yaml`, prek can still read it today.

Add a small YAML file so the first run has something to check, then stage both
files:

```yaml title="example.yaml"
project: prek
enabled: true
```

```bash
git add prek.toml example.yaml
```

`prek run` checks the staged snapshot, so a new or changed config must be staged
before this default run. You can still review or unstage it after trying the
workflow.

### 2. Run hooks on demand

Use `prek run` to execute all configured hooks on the files in your current git staging area:

```console
$ prek run
trim trailing whitespace.................................................Passed
fix end of files.........................................................Passed
check for added large files..............................................Passed
```

The first run can take longer because prek downloads the hook repository and
prepares its environment.

Need to run a single hook? Pass its ID, for example `prek run trailing-whitespace`. You can also target specific files with `--files`, or run against the entire repository with `--all-files`. Use `--all-files` after adding or changing a hook to check existing files that are not staged.

### 3. Customize the hooks

Edit the generated configuration to add or remove hooks. For example, add Ruff
to lint and format Python files:

```toml
[[repos]]
repo = "https://github.com/astral-sh/ruff-pre-commit"
rev = "v0.16.0"
hooks = [
  { id = "ruff-check" },
  { id = "ruff-format" },
]
```

Because `prek init` installed the Git shim, every `git commit` invokes the
configured hooks for the files in that commit. Run `prek install` to reinstall
the shim later, or `prek uninstall` to remove it.

### 4. Go further

- Explore richer configuration options in the official [pre-commit documentation](https://pre-commit.com/). Every example there works with prek.
- See [Common Workflows](./usage.md) for the commands you will use after setup and how to handle hook failures.
- Check the [configuration reference](./reference/configuration.md) for prek-specific settings.
- Browse the [built-in hooks](./builtin.md) and the [difference guide](./diff.md) to see what else you can leverage.

That’s it! You now have automated checks running locally with minimal setup. When you’re ready to dive deeper, the rest of the docs cover advanced workflows, language-specific installers, and more.
