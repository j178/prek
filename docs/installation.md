# Installation

prek provides multiple installation methods to suit different needs and environments.

## Standalone Installer

The standalone installer automatically downloads and installs the correct binary for your platform:

### Linux and macOS

```bash
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/j178/prek/releases/download/v0.2.0-alpha.4/prek-installer.sh | sh
```

### Windows

```powershell
powershell -ExecutionPolicy ByPass -c "irm https://github.com/j178/prek/releases/download/v0.2.0-alpha.4/prek-installer.ps1 | iex"
```

## Package Managers

### PyPI

Install via pip, uv (recommended), or pipx:

```bash
# Using uv (recommended)
uv tool install prek

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

To use prek with [mise](https://mise.jdx.dev):

```bash
mise use prek
```

## Build from Source

```bash
cargo install --locked --git https://github.com/j178/prek
```

## Download from GitHub Releases

Pre-built binaries are available for download from the [GitHub releases](https://github.com/j178/prek/releases) page.

## Updating

If you installed via the standalone installer, you can update to the latest version:

```bash
prek self update
```

For other installation methods, follow the same installation steps again.

## Shell Completion

prek supports shell completion for Bash, Zsh, Fish, and PowerShell. To install completions:

### Bash

```bash
COMPLETE=bash prek > /etc/bash_completion.d/prek
```

### Zsh

```bash
COMPLETE=zsh prek completion > "${fpath[1]}/_prek"
```

### Fish

```bash
COMPLETE=fish prek > ~/.config/fish/completions/prek.fish
```

### PowerShell

```powershell
COMPLETE=powershell prek >> $PROFILE
```
