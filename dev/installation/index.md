# Installation

prek provides multiple installation methods to suit different needs and environments.

Prebuilt releases are available for macOS, Linux, and Windows across the architectures listed on the [GitHub Releases](<https://github.com/j178/prek/releases>) page.

## Standalone Installer

The standalone installer automatically downloads and installs the correct binary for your platform:

Use `curl` to download the script and execute it with `sh`:

```bash
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/j178/prek/releases/download/v0.5.6/prek-installer.sh | sh
```

Use `irm` to download the script and execute it with `iex`:

```powershell
powershell -ExecutionPolicy ByPass -c "irm https://github.com/j178/prek/releases/download/v0.5.6/prek-installer.ps1 | iex"
```

Changing the [execution policy](<https://learn.microsoft.com/en-us/powershell/module/microsoft.powershell.core/about/about_execution_policies>) allows running a script from the internet.

> [!TIP]
>
> The installation script may be inspected before use. Alternatively, binaries can be downloaded directly from [GitHub Releases](<#github-releases>).

> [!NOTE]
>
> **Installer options**
>
> ### Changing the installation path
>
> By default, the standalone installer tries `$XDG_BIN_HOME`, then `$XDG_DATA_HOME/../bin`, and finally `~/.local/bin`. Set `PREK_INSTALL_DIR` to override these locations:
>
> ```bash
> curl --proto '=https' --tlsv1.2 -LsSf https://github.com/j178/prek/releases/download/v0.5.6/prek-installer.sh | env PREK_INSTALL_DIR="/custom/path" sh
> ```
>
> ```powershell
> powershell -ExecutionPolicy ByPass -c {$env:PREK_INSTALL_DIR = "C:\Custom\Path"; irm https://github.com/j178/prek/releases/download/v0.5.6/prek-installer.ps1 | iex}
> ```
>
> > **Note**
> >
> > `PREK_INSTALL_DIR` controls where the prek binary is installed. To change where caches, toolchains, and hook environments are stored, use [`PREK_HOME`](<https://prek.j178.dev/dev/reference/environment-variables/#prek_home>).
>
> ### Disabling PATH modifications
>
> The installer adds the installation directory to your `PATH` by updating shell profiles on macOS and Linux, or the user environment on Windows. Set `PREK_NO_MODIFY_PATH=1` to skip this step:
>
> ```bash
> curl --proto '=https' --tlsv1.2 -LsSf https://github.com/j178/prek/releases/download/v0.5.6/prek-installer.sh | env PREK_NO_MODIFY_PATH=1 sh
> ```
>
> ```powershell
> powershell -ExecutionPolicy ByPass -c {$env:PREK_NO_MODIFY_PATH = "1"; irm https://github.com/j178/prek/releases/download/v0.5.6/prek-installer.ps1 | iex}
> ```
>
> You can combine this with `PREK_INSTALL_DIR`. Add the directory to `PATH` yourself or invoke prek by its full path.
>
> ### Unmanaged installations
>
> For temporary environments such as CI, set `PREK_UNMANAGED_INSTALL` to the desired installation directory. This also skips PATH modifications and disables `prek self update`:
>
> ```bash
> curl --proto '=https' --tlsv1.2 -LsSf https://github.com/j178/prek/releases/download/v0.5.6/prek-installer.sh | env PREK_UNMANAGED_INSTALL="/custom/path" sh
> ```
>
> ```powershell
> powershell -ExecutionPolicy ByPass -c {$env:PREK_UNMANAGED_INSTALL = "C:\Custom\Path"; irm https://github.com/j178/prek/releases/download/v0.5.6/prek-installer.ps1 | iex}
> ```
>
> Add the directory to `PATH` yourself or invoke prek by its full path. To update an unmanaged installation, rerun the installer for the desired version.

## Package Managers

### PyPI

prek is published as Python binary wheel to PyPI, you can install it using `pip`, `uv` (recommended), or `pipx`:

```bash
# Using uv (recommended)
uv tool install prek

# Using uvx (install and run in one command)
uvx prek

# Adding prek to the project dev-dependencies
uv add --dev prek

# Using pip
pip install prek

# Using pipx
pipx install prek
```

### Homebrew (macOS/Linux)

```bash
brew install prek
```

### mise

To use prek with [mise](<https://mise.jdx.dev>) ([v2025.8.11](<https://github.com/jdx/mise/releases/tag/v2025.8.11>) or later):

```bash
mise use prek
```

### npm

prek is published as a [Node.js package](<https://www.npmjs.com/package/@j178/prek>) and can be installed with any npm-compatible package manager:

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

prek is available via [Nixpkgs](<https://search.nixos.org/packages?channel=unstable&amp;show=prek&amp;query=prek>).

```shell
# Choose what's appropriate for your use case.
# One-off in a shell:
nix-shell -p prek

# NixOS or non-NixOS without flakes:
nix-env -iA nixos.prek

# Non-NixOS with flakes:
nix profile install nixpkgs#prek
```

### Conda

prek is available as `prek` via [conda-forge](<https://anaconda.org/conda-forge/prek>).

```shell
conda install conda-forge::prek
```

### Scoop (Windows)

prek is available via [Scoop](<https://scoop.sh/#/apps?q=prek>).

```powershell
scoop install main/prek
```

### Winget (Windows)

prek is available via [winget](<https://learn.microsoft.com/en-us/windows/package-manager/winget/>).

```powershell
winget install --id j178.Prek
```

### MacPorts

prek is available via [MacPorts](<https://ports.macports.org/port/prek/>).

```bash
sudo port install prek
```

### cargo-binstall

Install pre-compiled binaries from GitHub using [cargo-binstall](<https://github.com/cargo-bins/cargo-binstall>):

```bash
cargo binstall prek
```

## Docker

prek provides a Docker image at [`ghcr.io/j178/prek`](<https://github.com/j178/prek/pkgs/container/prek>).

See the guide on [using prek in Docker](<https://prek.j178.dev/dev/integrations/#docker>) for more details.

## GitHub Releases

Pre-built binaries are available for download from the [GitHub releases](<https://github.com/j178/prek/releases>) page.

## Build from Source

Build from source using Cargo (Rust 1.97+ is required):

```bash
cargo install --locked prek
```

## Verify the installation

For a global installation, confirm that `prek` is available on your `PATH`:

```bash
prek --version
```

If you added prek as a project dependency, run it through that package manager:

```bash
uv run prek --version
```

```bash
npm exec -- prek --version
```

The rest of this documentation uses the shorter `prek` form. For a project dependency, substitute `uv run prek` or `npm exec -- prek`.

## Run without installing

To try prek in an isolated environment without adding it to your project, use `uvx`:

```bash
uvx prek --version
```

Use `uvx prek` in place of `prek` in subsequent commands.

## Updating

If installed via the standalone installer, prek can update itself to the latest version:

```bash
prek self update
```

If you installed prek with a package manager, use its upgrade command. For example, use `uv tool upgrade prek` for a uv tool installation or `pip install --upgrade prek` for a pip installation.

## Shell Completion

> [!TIP]
>
> Run `echo $SHELL` to determine your shell.

prek provides shell completion for commands and options. Bash, Zsh, Fish, PowerShell, and Elvish also support hook or project selectors. To generate and load the completion script when your shell starts, run one of the following:

```bash
echo 'eval "$(prek util generate-shell-completion bash)"' >> ~/.bashrc
```

```bash
echo 'eval "$(prek util generate-shell-completion zsh)"' >> ~/.zshrc
```

```fish
echo 'prek util generate-shell-completion fish | source' >> ~/.config/fish/config.fish
```

```powershell
Add-Content -Path $PROFILE -Value 'prek util generate-shell-completion powershell | Out-String | Invoke-Expression'
```

Save the completion script:

```text
prek util generate-shell-completion nushell | save --force ($nu.default-config-dir | path join 'prek-completions.nu')
```

Add the following to your `config.nu`:

```text
source ($nu.default-config-dir | path join 'prek-completions.nu')
```

Then restart your shell or source the config file.

You can also save the generated script to a completion file. Regenerate it after upgrading prek so the script matches the installed version.

## Artifact Verification

Release artifacts are signed with [GitHub Attestations](<https://docs.github.com/en/actions/security-for-github-actions/using-artifact-attestations>) to provide cryptographic proof of their origin. Verify downloads using the [GitHub CLI](<https://cli.github.com/>):

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
