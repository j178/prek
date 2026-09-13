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

| What the command needs | Good starting point | Runtime source |
| -- | -- | -- |
| A tool already installed by the project or CI image | [`system`](reference/language-support.md#system) | Your `PATH`; prek does not install it |
| A checked-in executable script with no isolated dependencies | [`script`](reference/language-support.md#script) | The hook repository or local project |
| An isolated ecosystem environment | Python, Node, Bun, Deno, .NET, Go, mise, Ruby, or Rust | prek can select and download a compatible toolchain |
| An ecosystem currently supplied by the machine | Conda, Coursier, Dart, Haskell, Julia, Lua, Perl, PHP, R, or Swift | A matching system installation |
| A fully packaged runtime | [`docker`](reference/language-support.md#docker) or [`docker_image`](reference/language-support.md#docker_image) | A supported container runtime |
| A message-only failure or content regex | [`fail`](reference/language-support.md#fail), [`pygrep`](reference/language-support.md#pygrep), or a [builtin hook](reference/built-in-hooks.md) | No general-purpose hook environment |

For an existing project linter or formatter, begin with
[`language = "system"`](reference/language-support.md#system). Choose a managed language when the hook
repository itself needs an isolated installation or when prek should select the
toolchain version.

For supported versions, dependencies, and installation behavior, see the
[Language Support reference](reference/language-support.md).

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

Use file filters to avoid starting a command when no relevant file changed:

- `types` and `types_or` use file type tags detected by prek.
- `files` and `exclude` match paths with regular expressions, or with prek's
  explicit glob form.
- `stages` limits the Git hook stages where a hook is eligible.

Inspect a file's detected tags with:

```bash
prek util identify path/to/file
```

The [configuration reference](reference/configuration.md#common-hook-options)
documents how the filters combine.

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
