# Continuous Integration

Run the same checked-in configuration locally and in CI. For most repositories,
the reliable default is:

```bash
prek run --all-files
```

This checks every tracked file instead of depending on a CI job's staging area.
The command exits unsuccessfully when a hook fails or modifies files, so no
extra wrapper is required.

## GitHub Actions

The official [`j178/prek-action`](https://github.com/j178/prek-action) installs
prek and runs `prek run --all-files`. See the ready-to-copy workflow in
[Integrations](integrations.md#github-actions).

## Other CI systems

Install a pinned prek version using one of the methods in the
[Installation](installation.md) guide, check out the repository, and run:

```bash
prek run --all-files
```

Project-local commands still need their project dependencies. For example, a
local hook that invokes `npm exec` requires the Node dependencies to be
installed before prek runs.

## Check only a revision range

Large repositories can run hooks only for files changed between two refs:

```bash
prek run --from-ref origin/main --to-ref HEAD
```

The checkout must contain both refs and enough history to calculate the diff.
Shallow CI checkouts often need a larger fetch depth or an explicit fetch of the
base branch. If that setup is unreliable, use `--all-files`.

## Validate updates without changing the config

Repository maintainers can check whether pinned hook revisions are outdated:

```bash
prek update --check
```

Use [`prek validate-config`](reference/cli.md#prek-validate-config) after editing
a config, and use [`prek validate-manifest`](reference/cli.md#prek-validate-manifest)
in repositories that publish hooks.

## Cache and credentials

`PREK_HOME` contains cloned hook repositories, prepared environments, managed
toolchains, and logs. Caching it can reduce setup time, but cache correctness
depends on the prek version, platform, config, and hook revisions. Start without
a cache, then add a narrowly keyed cache only if environment preparation is a
meaningful part of the job.

Private hook repositories need non-interactive Git credentials. Configure the
CI provider's credential helper or token before running prek, and avoid printing
tokens in verbose logs. See the [private repository FAQ](faq.md#how-do-i-use-hooks-from-private-repositories)
and the [Security Guide](security.md).
