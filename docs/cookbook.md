# Cookbook

Short recipes for setup patterns that go beyond the default project-local workflow.

## Enable a Global Hook with Git Config

Git 2.54 introduced [config-based hooks](https://github.blog/open-source/git/highlights-from-git-2-54/#h-config-based-hooks), which let Git run hooks from config instead of hook scripts.
This is useful when you want a personal `prek` hook that works across repositories.

Choose the Git hook event you want to run on, for example `pre-commit`, then register a global config-based hook:

```bash
git config --global hook.prek-pre-commit.event pre-commit
git config --global hook.prek-pre-commit.command 'prek hook-impl --hook-type pre-commit --skip-on-missing-config'
```

The config has three moving parts:

- `hook.<friendly-name>.event`: the Git hook event to listen for, such as `pre-commit`, `pre-push`, or `commit-msg`.
- `hook.<friendly-name>.command`: the command Git runs for that event.
- `<friendly-name>`: a user-defined name for this configured hook. Keep it unique in your Git config.

Keep `--skip-on-missing-config` in the command so repositories without a `prek.toml` or `.pre-commit-config.yaml` do not fail ordinary Git operations.

To configure a different event, replace both occurrences of `pre-commit`:

```bash
git config --global hook.<friendly-name>.event <event>
git config --global hook.<friendly-name>.command 'prek hook-impl --hook-type <event> --skip-on-missing-config'
```

By default, `prek hook-impl` discovers the current repository's config.
If you want one global hook config to run in every repository, pass that config explicitly:

```bash
git config --global hook.<friendly-name>.command 'prek hook-impl --hook-type <event> --config <config-file>'
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
git config --global hook.gitleaks.command 'prek hook-impl --hook-type pre-commit --config ~/.config/prek/global-hooks.toml'
```
