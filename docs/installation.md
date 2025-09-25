# Installation

prek provides multiple installation methods to suit different needs and environments.

## Standalone Installer

The standalone installer automatically downloads and installs the correct binary for your platform:

### Linux and macOS

{%
  include-markdown "../README.md"
  start="<!-- linux-standalone-install:start -->"
  end="<!-- linux-standalone-install:end -->"
%}

### Windows

{%
  include-markdown "../README.md"
  start="<!-- windows-standalone-install:start -->"
  end="<!-- windows-standalone-install:end -->"
%}

## Package Managers

### PyPI

Install via pip, uv (recommended), or pipx:

{%
  include-markdown "../README.md"
  start="<!-- pypi-install:start -->"
  end="<!-- pypi-install:end -->"
%}

### Homebrew (macOS/Linux)

{%
  include-markdown "../README.md"
  start="<!-- homebrew-install:start -->"
  end="<!-- homebrew-install:end -->"
%}

### mise

{%
  include-markdown "../README.md"
  start="<!-- mise-install:start -->"
  end="<!-- mise-install:end -->"
%}

## Build from Source

{%
  include-markdown "../README.md"
  start="<!-- cargo-install:start -->"
  end="<!-- cargo-install:end -->"
%}

## Download from GitHub Releases

{%
  include-markdown "../README.md"
  start="<!-- pre-built-binaries:start -->"
  end="<!-- pre-built-binaries:end -->"
%}

## Updating

{%
  include-markdown "../README.md"
  start="<!-- self-update:start -->"
  end="<!-- self-update:end -->"
%}

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

## Use in GitHub Actions

{%
  include-markdown "../README.md"
  start="<!-- github-actions:start -->"
  end="<!-- github-actions:end -->"
%}
