# FAQ

## How is `prek` pronounced?

Like "wreck", but with a "p" sound instead of the "w" at the beginning.

## I updated `.prekignore`, why didn't discovery change?

Workspace discovery is cached. If you edited `.prekignore`, run the command with `--refresh` to force a fresh project discovery so the changes are picked up. For example:

```bash
prek run --refresh
```

## What does `prek install --install-hooks` do?

In short, it installs the Git hooks **and** prepares the environments for the hooks managed by prek. It is inherited from the original Python-based `pre-commit` tool (I'll abbreviate it as **ppc** in this document) to maintain compatibility with existing workflows.

It's a little confusing because it refers to two different kinds of hooks:

1. **Git hooks** – Scripts placed inside `.git/hooks/`, such as `.git/hooks/pre-commit`, that Git executes during lifecycle events. Both prek and ppc drop a small shim here so Git automatically runs them on `git commit`.
2. **prek-managed hooks** – The tools listed in `.pre-commit-config.yaml`. When prek runs, it executes these hooks and prepares whatever runtime they need (for example, creating a Python virtual environment and installing the hook's dependencies before execution).

Running `prek install` installs the first type: it writes the Git hook so that Git knows to call prek. Adding `--install-hooks` tells prek to do that **and** proactively create the environments and caches required by the hooks that prek manages. That way, the next time the Git hook fires, the managed hooks are ready to run without additional setup.

## How do I use hooks from private repositories?

prek supports cloning hooks from private repositories that require authentication.
Since prek disables interactive terminal prompts (to prevent CI hangs), you'll need
to configure credentials via credential helpers, environment variables, or SSH.

### Option 1: Credential helpers (recommended)

If you use GitHub CLI, Git Credential Manager, macOS Keychain, or similar tools,
authentication often works automatically with no extra configuration:

```shell
# GitHub CLI users: configure git to use gh for credentials
gh auth setup-git

# Now HTTPS URLs work automatically
prek install
```

Other credential helpers that work out of the box:

- **macOS**: Keychain (`credential.helper=osxkeychain`)
- **Windows**: Git Credential Manager (`credential.helper=manager`)
- **Linux**: GNOME Keyring, KWallet, or `credential.helper=store`

You can also use `GIT_ASKPASS` to point to a custom credential program:

```shell
export GIT_ASKPASS=/path/to/credential-script
```

### Option 2: SSH URLs

Use SSH URLs in your `.pre-commit-config.yaml` instead of HTTPS:

```yaml
repos:
  - repo: git@github.com:myorg/private-hooks.git
    rev: v1.0.0
    hooks:
      - id: my-hook
```

This works automatically if you have SSH keys configured with an agent.

### Option 3: URL rewriting with tokens (for CI)

In CI environments without credential helpers, use environment variables to
rewrite HTTPS URLs to include credentials:

```shell
# GitHub Actions example
export GIT_CONFIG_COUNT=1
export GIT_CONFIG_KEY_0="url.https://oauth2:${GITHUB_TOKEN}@github.com/.insteadOf"
export GIT_CONFIG_VALUE_0="https://github.com/"

# Or using GIT_CONFIG_PARAMETERS (more compact)
export GIT_CONFIG_PARAMETERS="'url.https://oauth2:${GITHUB_TOKEN}@github.com/.insteadOf=https://github.com/'"
```

> **Security note:** Be careful with tokens in environment variables. Ensure your
> CI system masks secrets in logs.
