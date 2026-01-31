# Installation

prek provides multiple installation methods to suit different needs and environments.

## Standalone Installer

The standalone installer automatically downloads and installs the correct binary for your platform:

### Linux and macOS

--8<-- "README.md:linux-standalone-install"

### Windows

--8<-- "README.md:windows-standalone-install"

## Package Managers

### PyPI

--8<-- "README.md:pypi-install"

### Homebrew (macOS/Linux)

--8<-- "README.md:homebrew-install"

### mise

--8<-- "README.md:mise-install"

### npmjs

--8<-- "README.md:npmjs-install"

### Nix

--8<-- "README.md:nix-install"

### Conda

--8<-- "README.md:conda-forge-install"

### Scoop (Windows)

--8<-- "README.md:scoop-install"

### MacPorts (macOS)

--8<-- "README.md:macports-install"

### Install from Pre-Built Binaries

--8<-- "README.md:cargo-binstall"

## Build from Source

--8<-- "README.md:cargo-install"

## Download from GitHub Releases

--8<-- "README.md:pre-built-binaries"

## Updating

--8<-- "README.md:self-update"

For other installation methods, follow the same installation steps again.

## Shell Completion

prek supports shell completion for Bash, Zsh, Fish, and PowerShell. To install completions:

### Bash

```bash
COMPLETE=bash prek > /etc/bash_completion.d/prek
```

### Zsh

```bash
COMPLETE=zsh prek > "${fpath[1]}/_prek"
```

### Fish

```bash
COMPLETE=fish prek > ~/.config/fish/completions/prek.fish
```

### PowerShell

```powershell
COMPLETE=powershell prek >> $PROFILE
```

## Artifact Verification

Release artifacts are signed with
[GitHub Attestations](https://docs.github.com/en/actions/security-for-github-actions/using-artifact-attestations)
to provide cryptographic proof of their origin. Verify downloads using the
[GitHub CLI](https://cli.github.com/):

```console
$ gh attestation verify prek-x86_64-unknown-linux-gnu.tar.gz --repo j178/prek
Loaded digest sha256:xxxx... for file://prek-x86_64-unknown-linux-gnu.tar.gz
Loaded 1 attestation from GitHub API
✓ Verification succeeded!

- Attestation #1
  - Build repo:..... j178/prek
  - Build workflow:. .github/workflows/release.yml@refs/tags/vX.Y.Z
```

This confirms the artifact was built by the official release workflow.
