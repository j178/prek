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

## Automatically fix pull requests with autofix.ci

[`autofix.ci`](https://autofix.ci/) can commit changes made by formatting and
other fixing hooks back to a pull request. It cannot fix a check-only failure;
the configured hook must modify files itself.

Install the [autofix.ci GitHub App](https://autofix.ci/setup), then add
`.github/workflows/autofix.yml`:

```yaml
name: autofix.ci

on:
  pull_request:
  push:
    branches: [main]

permissions:
  contents: read

jobs:
  autofix:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7
      - uses: j178/prek-action@4e14d07f9231acabce116ccfca13b13dd9755ece # v3.0.0
        with:
          install-only: true

      - name: Run prek
        id: prek
        continue-on-error: true
        run: prek run --all-files

      - name: Verify fixes
        if: steps.prek.outcome == 'failure'
        run: prek run --all-files

      - name: Commit fixes
        if: always() && !cancelled()
        uses: autofix-ci/action@c5b2d67aa2274e7b5a18224e8171550871fc7e4a # v1.3.4
```

Keep the workflow name exactly `autofix.ci`; the service uses it to identify
the trusted workflow. The first prek run may fail after a hook changes files,
so the workflow lets that step continue and runs prek again against the updated
working tree. The final step still records those changes when another check
cannot be fixed, while the failed verification keeps the job unsuccessful.

Run all fixing tools in this job and call `autofix-ci/action` only once, after
they finish. The workflow itself keeps read-only repository access; the GitHub
App provides the scoped permission used to create the fix commit. See the
[autofix.ci security model](https://autofix.ci/security) for details.

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
