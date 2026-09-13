# Local Hooks

`repo: local` defines hooks directly in the current project's configuration
instead of loading them from a separate hook repository.

## A minimal local hook

The following hook expects `uv` and the project's dependencies to be available.
`prek` appends matching Python filenames to the command.

=== "prek.toml"

    ```toml
    [[repos]]
    repo = "local"

    [[repos.hooks]]
    id = "ruff"
    name = "ruff"
    language = "system"
    entry = "uv run ruff check"
    types = ["python"]
    ```

=== ".pre-commit-config.yaml"

    ```yaml
    repos:
      - repo: local
        hooks:
          - id: ruff
            name: ruff
            language: system
            entry: uv run ruff check
            types: [python]
    ```

`language = "system"` means that prek does not install the command. The entry
and any interpreters or package managers it invokes must already be available
on `PATH`.

## Choose a language

For a project linter or formatter, start with `language = "system"` as in the
example above. Other choices depend on how the command should be installed:

| What the command needs | Language |
| -- | -- |
| A tool already installed by the project or CI image | [`system`](reference/language-support.md#system) |
| A checked-in executable script | [`script`](reference/language-support.md#script) |
| Dependencies installed in a hook environment | The ecosystem's language, such as `python` or `node` |
| A packaged container runtime | [`docker`](reference/language-support.md#docker) or [`docker_image`](reference/language-support.md#docker_image) |

The [Language Support reference](reference/language-support.md) lists supported
languages, toolchain requirements, and installation behavior. For simple content
or filename checks, a [built-in hook](reference/built-in-hooks.md) may already do
what you need.

## Decide how the command receives files

`pass_filenames` defaults to `true`. Matching filenames are appended after
`entry` and `args`:

```text
uv run ruff check path/to/one.py path/to/two.py
```

Set it to `false` when the command discovers files itself or always checks a
whole workspace:

```toml
[[repos]]
repo = "local"

[[repos.hooks]]
id = "cargo-fmt"
name = "cargo fmt"
language = "system"
entry = "cargo fmt --all -- --check"
types = ["rust"]
pass_filenames = false
```

A positive integer limits each invocation to that many filenames and lets prek
split a large match set into batches. See
[`pass_filenames`](reference/configuration.md#pass_filenames) before using this
prek-specific form.

## Filter when the hook runs

Local hooks use the same [file and stage filters](configuration.md#choose-which-files-and-stages-to-check)
as remote hooks. In the Ruff example, `types = ["python"]` limits the command to
Python files. In the Cargo example, `types = ["rust"]` controls whether the
command runs, while `pass_filenames = false` lets Cargo select files itself.

## Commands do not use a shell by default

prek splits `entry` into arguments and invokes the program directly. Operators
such as `|`, `&&`, redirects, variables, and globs are not interpreted by a
shell.

Prefer putting complex logic in a checked-in script and using that script as
the entry. If shell syntax is truly part of the hook, set the prek-specific
[`shell`](reference/configuration.md#shell) option and write the command for
that shell. Shell-specific hooks are less portable, especially between Windows
and Unix systems.

## Working directory

A local hook runs in the directory of the project whose config defines it. In a
single-config repository this is normally the Git root. In
[workspace mode](monorepos.md), a nested project's hooks run in that nested
project directory. Entries should therefore use paths relative to their own
project rather than the directory from which the user invoked prek.

For the exact entry resolution model, see
[Hook Entry Resolution](internals.md#hook-entry-resolution).
