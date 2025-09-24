# prek

<div align="center">
  <img width="220" alt="prek" src="/assets/logo_512.webp" />
</div>

{%
  include-markdown "../README.md"
  start="<!-- description:start -->"
  end="<!-- description:end -->"
%}

!!! warning "Not production-ready yet"
    {%
      include-markdown "../README.md"
      start="<!-- warning-p1:start -->"
      end="<!-- warning-p1:end -->"
    %}

    {%
      include-markdown "../README.md"
      start="<!-- warning-p2:start -->"
      end="<!-- warning-p2:end -->"
    %}

{%
  include-markdown "../README.md"
  start="<!-- features:start -->"
  end="<!-- features:end -->"
%}

{%
  include-markdown "../README.md"
  start="<!-- why:start -->"
  end="<!-- why:end -->"
%}

## Getting Started

- [Installation](./installation.md) - Installation options
- [Workspace Mode](./workspace.md) - Monorepo support
- [Differences](./diff.md) - What's different from pre-commit
- [Debugging](./debugging.md) - Troubleshooting tips
