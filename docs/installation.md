# Installation

prek provides multiple installation methods to suit different needs and environments.

Prebuilt releases are available for macOS, Linux, and Windows
across the architectures listed on the
[GitHub Releases](https://github.com/j178/prek/releases) page.

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

### Winget (Windows)

--8<-- "README.md:winget-install"

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

## Verify the installation

For a global installation, confirm that `prek` is available on your `PATH`:

```bash
prek --version
```

If you added prek as a project dependency, run it through that package manager:

=== "uv project dependency"

    ```bash
    uv run prek --version
    ```

=== "npm project dependency"

    ```bash
    npm exec -- prek --version
    ```

The rest of this documentation uses the shorter `prek` form. For a project
dependency, substitute `uv run prek` or `npm exec -- prek`.

## Run without installing

To try prek in an isolated environment without adding it to your project, use
`uvx`:

```bash
uvx prek --version
```

Use `uvx prek` in place of `prek` in subsequent commands.

## Updating

--8<-- "README.md:self-update"

If you installed prek with a package manager, use its upgrade command. For
example, use `uv tool upgrade prek` for a uv tool installation or
`pip install --upgrade prek` for a pip installation.

## Shell Completion

!!! tip

    Run `echo $SHELL` to determine your shell.

prek provides shell completion for commands, options, and hook or project selectors.
To generate and load the completion script when your shell starts, run one of the following:

=== "Bash"

    ```bash
    echo 'eval "$(prek util generate-shell-completion bash)"' >> ~/.bashrc
    ```

=== "Zsh"

    ```bash
    echo 'eval "$(prek util generate-shell-completion zsh)"' >> ~/.zshrc
    ```

=== "Fish"

    ```fish
    echo 'prek util generate-shell-completion fish | source' >> ~/.config/fish/config.fish
    ```

=== "PowerShell"

    ```powershell
    Add-Content -Path $PROFILE -Value 'prek util generate-shell-completion powershell | Out-String | Invoke-Expression'
    ```

Then restart your shell or source the config file.

You can also save the generated script to a completion file. Regenerate it after
upgrading prek so the script matches the installed version.

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
