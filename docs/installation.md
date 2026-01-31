# Installation

prek provides multiple installation methods to suit different needs and environments.

## Standalone Installer

The standalone installer automatically downloads and installs the correct binary for your platform:

=== "macOS and Linux"

    Use `curl` to download the script and execute it with `sh`:

    --8<-- "README.md:linux-standalone-install"

=== "Windows"

    Use `irm` to download the script and execute it with `iex`:

    --8<-- "README.md:windows-standalone-install"

    Changing the [execution policy](https://learn.microsoft.com/en-us/powershell/module/microsoft.powershell.core/about/about_execution_policies) allows running a script from the internet.

!!! tip

    The installation script may be inspected before use. Alternatively, binaries can be downloaded directly from [GitHub Releases](#github-releases).

## Package Managers

### PyPI

--8<-- "README.md:pypi-install"

### Homebrew (macOS/Linux)

--8<-- "README.md:homebrew-install"

### mise

--8<-- "README.md:mise-install"

### npm

prek is published as a [Node.js package](https://www.npmjs.com/package/@j178/prek)
and can be installed with any npm-compatible package manager:

```bash
# npm
npm install -g @j178/prek

# pnpm
pnpm add -g @j178/prek

# bun
bun install -g @j178/prek
```

Or as a project dependency:

```bash
npm add -D @j178/prek
```

### Nix

--8<-- "README.md:nix-install"

### Conda

--8<-- "README.md:conda-forge-install"

### Scoop (Windows)

--8<-- "README.md:scoop-install"

### MacPorts

--8<-- "README.md:macports-install"

### cargo-binstall

--8<-- "README.md:cargo-binstall"

## Docker

prek provides a Docker image at
[`ghcr.io/j178/prek`](https://github.com/j178/prek/pkgs/container/prek).

See the guide on [using prek in Docker](integrations.md#docker) for more details.

## GitHub Releases

--8<-- "README.md:pre-built-binaries"

## Build from Source

--8<-- "README.md:cargo-install"

## Updating

--8<-- "README.md:self-update"

For other installation methods, follow the same installation steps again.

## Shell Completion

!!! tip

    Run `echo $SHELL` to determine your shell.

To enable shell autocompletion for prek commands, run one of the following:

=== "Bash"

    ```bash
    echo 'eval "$(COMPLETE=bash prek)"' >> ~/.bashrc
    ```

=== "Zsh"

    ```bash
    echo 'eval "$(COMPLETE=zsh prek)"' >> ~/.zshrc
    ```

=== "Fish"

    ```bash
    echo 'COMPLETE=fish prek | source' >> ~/.config/fish/config.fish
    ```

=== "PowerShell"

    ```powershell
    Add-Content -Path $PROFILE -Value '(COMPLETE=powershell prek) | Out-String | Invoke-Expression'
    ```

Then restart your shell or source the config file.

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
