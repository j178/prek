# Security Guide

A hook configuration is executable project code. Installing a Git shim or
running `prek run` authorizes the selected hooks, their installation steps, and
their dependencies to execute with the current user's permissions.

## Review configurations before running them

Treat an unfamiliar `prek.toml` or `.pre-commit-config.yaml` like an unfamiliar
build script:

- Review remote repository URLs, revisions, hook entries, and additional
  dependencies.
- Review local hooks and checked-in scripts, especially entries that explicitly
  invoke a shell.
- Inspect configuration changes in pull requests before running them locally.
- Use tighter credentials and isolation for untrusted repositories.

This is especially important for a global Git hook that discovers and runs each
repository's config automatically. Such a hook can execute project-controlled
code during an ordinary Git operation.

## Pin remote hooks

A full commit SHA is the strongest Git pin because it names immutable content.
A version tag is easier to read and is commonly used for releases, but Git tags
can be moved by a repository maintainer. Branch names are mutable and should not
be treated as repeatable pins.

Use `prek update --freeze` to write commit SHAs when immutable pins are required.
Review the resulting revision changes before committing them.

## Downloads and checksums

For managed download paths that support checksum sidecars, prek verifies the
download when the upstream source provides that metadata. The
[download checksum policy](reference/environment-variables.md#prek_download_checksum_policy)
controls what happens when metadata is missing; it cannot create authenticity
information that an upstream source did not publish.

Release artifacts for prek itself include GitHub attestations. See
[Artifact Verification](installation.md#artifact-verification).

## Credentials and logs

Use a credential helper or short-lived CI token for private hook repositories.
Avoid embedding credentials in repository URLs or configuration files. Verbose
hook output and `prek.log` can contain repository paths, command arguments, and
tool output, so review and redact logs before sharing them publicly.
