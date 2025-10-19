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

2. Reinstall the git hooks once via `prek install -f` (run this if you previously executed `pre-commit install`).

From here you can explore what prek adds on top of pre-commit:

- [Key differences and new features](./diff.md)
- [Built-in Rust-native hooks](./builtin.md)
- [Workspace mode for monorepos](./workspace.md)

## New to pre-commit-style workflows?

Follow this short example to experience how prek automates linting and formatting tasks.

### 1. Create a configuration

In the root of your repository, add a `.pre-commit-config.yaml`:

```yaml
repos:
  - repo: https://github.com/pre-commit/pre-commit-hooks
    rev: v6.0.0
    hooks:
      - id: check-yaml
      - id: end-of-file-fixer
```

This config downloads a couple of community-maintained hooks that validate YAML files and ensure files end with a newline.

### 2. Run hooks on demand

Use `prek run` to execute all configured hooks on the files in your current git staging area:

```bash
prek run
```

Need to run a single hook? Pass its ID, for example `prek run check-yaml`. You can also target specific files with `--files`, or run against the entire repository with `--all-files`.

### 3. Wire hooks into git automatically

To run the hooks every time you commit, install prek’s git hook integration:

```bash
prek install
```

Now every `git commit` will invoke `prek run` for the files included in the commit. If you ever want to undo this, run `prek uninstall`.

### 4. Go further

- Explore richer configuration options in the official [pre-commit documentation](https://pre-commit.com/). Every example there works with prek.
- Check the [configuration reference](./configuration.md) for prek-specific settings.
- Browse the [built-in hooks](./builtin.md) and the [difference guide](./diff.md) to see what else you can leverage.

That’s it! You now have automated checks running locally with minimal setup. When you’re ready to dive deeper, the rest of the docs cover advanced workflows, language-specific installers, and more.
