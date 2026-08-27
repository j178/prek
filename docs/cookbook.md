# Cookbook

Short recipes for setup patterns that go beyond the default project-local workflow.

## Enable a Global Hook with Git Config

Git 2.54 introduced [config-based hooks](https://github.blog/open-source/git/highlights-from-git-2-54/#h-config-based-hooks), which let Git run hooks from config instead of hook scripts.
This is useful when you want a personal `prek` hook that works across repositories.

!!! warning "Only enable discovery for repositories you trust"

    The discovery form below reads the current repository's config and executes
    its hooks during Git operations. A repository can therefore cause
    project-controlled code to run with your user permissions. Use this pattern
    only for trusted repositories. For broader use, point the hook at a fixed
    config that you control and review the [Security Guide](security.md).

Confirm that your Git version supports config-based hooks:

```bash
git --version
```

The version must be 2.54 or newer.

Choose the Git hook event you want to run on, for example `pre-commit`, then register a global config-based hook:

=== "git config command"

    ```bash
    git config --global hook.prek-pre-commit.event pre-commit
    git config --global hook.prek-pre-commit.command 'prek hook-impl --hook-type pre-commit --skip-on-missing-config --'
    ```

=== "gitconfig file"

    Edit your global Git config directly, for example in `~/.gitconfig`:

    ```gitconfig
    [hook "prek-pre-commit"]
        event = pre-commit
        command = prek hook-impl --hook-type pre-commit --skip-on-missing-config --
    ```

The config has three moving parts:

- `hook.<friendly-name>.event`: the Git hook event to listen for, such as `pre-commit`, `pre-push`, or `commit-msg`.
- `hook.<friendly-name>.command`: the command Git runs for that event.
- `<friendly-name>`: a user-defined name for this configured hook. Keep it unique in your Git config.

!!! tip "Keep these command options"

    Keep `--skip-on-missing-config` in the command so repositories without a `prek.toml` or `.pre-commit-config.yaml` do not fail ordinary Git operations.

    Keep the trailing `--` so Git-provided hook arguments, such as a `commit-msg` filename or `pre-push` remote name and URL, are forwarded to `prek hook-impl` instead of being parsed as hook selectors.

By default, `prek hook-impl` discovers the current repository's config.
If you want one global hook config to run in every repository, pass that config explicitly:

```bash
git config --global hook.<friendly-name>.command 'prek hook-impl --hook-type <event> --config <config-file> --'
```

For example, a global config file at `~/.config/prek/global-hooks.toml` can run gitleaks in every repository:

```toml
[[repos]]
repo = "https://github.com/gitleaks/gitleaks"
rev = "v8.24.2"
hooks = [{ id = "gitleaks" }]
```

Then point the global Git hook at that config:

```bash
git config --global hook.gitleaks.event pre-commit
git config --global hook.gitleaks.command 'prek hook-impl --hook-type pre-commit --config ~/.config/prek/global-hooks.toml --'
```

### Remove a global config-based hook

Remove both keys for the friendly name you registered. For the first example on
this page, run:

```bash
git config --global --unset-all hook.prek-pre-commit.event
git config --global --unset-all hook.prek-pre-commit.command
```

For a different friendly name, replace `prek-pre-commit` in both commands. This
changes the global Git configuration; it does not remove project-local hook
scripts installed by `prek install`.

## More recipes

- [Run an existing project linter or formatter](local-hooks.md)
- [Run hooks in continuous integration](ci.md)
- [Migrate while preserving an existing Git hook](migration.md#keep-the-existing-hook-during-rollout)
