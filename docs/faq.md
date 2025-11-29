# FAQ

## What does `prek install --install-hooks` do?

In short, it installs the Git hooks **and** prepares the environments for the hooks managed by prek. It is inherited from the original Python-based `pre-commit` tool (I'll abbreviate it as **ppc** in this document) to maintain compatibility with existing workflows.

It's a little confusing because it refers to two different kinds of hooks:

1. **Git hooks** – Scripts placed inside `.git/hooks/`, such as `.git/hooks/pre-commit`, that Git executes during lifecycle events. Both prek and ppc drop a small shim here so Git automatically runs them on `git commit`.
2. **prek-managed hooks** – The tools listed in `.pre-commit-config.yaml`. When prek runs, it executes these hooks and prepares whatever runtime they need (for example, creating a Python virtual environment and installing the hook's dependencies before execution).

Running `prek install` installs the first type: it writes the Git hook so that Git knows to call prek. Adding `--install-hooks` tells prek to do that **and** proactively create the environments and caches required by the hooks that prek manages. That way, the next time the Git hook fires, the managed hooks are ready to run without additional setup.
