# prek

<div align="center">
  <img width="220" alt="prek" src="/assets/logo.webp" />
</div>

{%
  include-markdown "../README.md"
  start="<!-- description:start -->"
  end="<!-- description:end -->"
%}

!!! note

    Although prek is pretty new, it’s already powering real‑world projects like [Apache Airflow](https://github.com/apache/airflow), and more projects are picking it up—see [Who is using prek?](#who-is-using-prek). If you’re looking for an alternative to `pre-commit`, please give it a try—we’d love your feedback!

    Please note that some subcommands and languages are still missing for full drop‑in parity with `pre-commit`. Track the remaining gaps here: [TODO](https://prek.j178.dev/todo/).

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

## Badges

Show that your project uses prek with a badge in your README:

[![prek](https://img.shields.io/endpoint?url=https://raw.githubusercontent.com/j178/prek/master/docs/assets/badge-v0.json)](https://github.com/j178/prek)

**Markdown**

```markdown
[![prek](https://img.shields.io/endpoint?url=https://raw.githubusercontent.com/j178/prek/master/docs/assets/badge-v0.json)](https://github.com/j178/prek)
```

**HTML**

```html
<a href="https://github.com/j178/prek">
  <img src="https://img.shields.io/endpoint?url=https://raw.githubusercontent.com/j178/prek/master/docs/assets/badge-v0.json" alt="prek">
</a>
```

**reStructuredText (RST)**

```rst
.. image:: https://img.shields.io/endpoint?url=https://raw.githubusercontent.com/j178/prek/master/docs/assets/badge-v0.json
   :target: https://github.com/j178/prek
   :alt: prek
```

## Getting Started

- [Installation](./installation.md) - How to install prek
- [Quickstart](./quickstart.md) - Getting started with prek
- [Configuration](./configuration.md) - Configuring prek
- [Workspace Mode](./workspace.md) - Monorepo support
- [Differences](./diff.md) - What's different from pre-commit
- [Debugging](./debugging.md) - Troubleshooting tips
