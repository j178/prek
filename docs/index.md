# prek

<div align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="/assets/logo-dark.png">
    <img alt="prek" src="/assets/logo.png" />
  </picture>
</div>

--8<-- "README.md:description"

!!! note

    Although prek is pretty new, it's already powering real‑world projects like [CPython](https://github.com/python/cpython), [Apache Airflow](https://github.com/apache/airflow), [FastAPI](https://github.com/fastapi/fastapi), and more projects are picking it up—see [Who is using prek?](#who-is-using-prek). If you're looking for an alternative to `pre-commit`, please give it a try—we'd love your feedback!

    Please note that some languages are not yet supported for full drop‑in parity with `pre-commit`. See [Language Support](https://prek.j178.dev/languages/) for current status.

--8<-- "README.md:features"

## Where to Start

- New to `prek`: start with [Installation](installation.md), then follow the [Quickstart](quickstart.md).
- Already set up: use [Common Workflows](usage.md) for the commands you run day to day.
- Writing config: read [Configuration](configuration.md), then use the [Configuration Reference](reference/configuration.md) for exact keys.
- Working in a monorepo: see [Workspace Mode](workspace.md).
- Looking for flags or environment variables: use the [CLI Reference](reference/cli.md) and [Environment Variable Reference](reference/environment-variables.md).

--8<-- "README.md:why"

## Badges

Show that your project uses prek with a badge in your README:

[![prek](https://img.shields.io/endpoint?url=https://raw.githubusercontent.com/j178/prek/master/docs/assets/badge-v0.json)](https://github.com/j178/prek)

=== "Markdown"

    ```markdown
    [![prek](https://img.shields.io/endpoint?url=https://raw.githubusercontent.com/j178/prek/master/docs/assets/badge-v0.json)](https://github.com/j178/prek)
    ```

=== "HTML"

    ```html
    <a href="https://github.com/j178/prek">
      <img src="https://img.shields.io/endpoint?url=https://raw.githubusercontent.com/j178/prek/master/docs/assets/badge-v0.json" alt="prek">
    </a>
    ```

=== "reStructuredText (RST)"

    ```rst
    .. image:: https://img.shields.io/endpoint?url=https://raw.githubusercontent.com/j178/prek/master/docs/assets/badge-v0.json
       :target: https://github.com/j178/prek
       :alt: prek
    ```
