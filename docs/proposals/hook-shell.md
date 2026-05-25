# Explicit shell execution for hook entries

This document outlines the design for running hook `entry` commands through an
explicit shell adapter.

## Motivation

`prek` currently runs hook entries as subprocesses without a shell. The `entry`
string is parsed into argv, then `args` and matching filenames are appended to
that argv. This is predictable and avoids ambient-shell differences, but it is
surprising for users who write hook entries as shell snippets:

```yaml
entry: uv run mypy || uv run pyright
```

The command above looks like a shell command, but without shell execution the
operators are passed as argv tokens to the process. Multiline entries have a
similar problem: formatting an `entry` as a YAML block scalar should not, by
itself, change how the command is executed.

The goal is to add shell execution as an explicit opt-in while preserving the
current no-shell default.

## Configuration

### Hook Configuration: `shell`

A new optional field `shell` is added to hook options.

```yaml
repos:
  - repo: local
    hooks:
      - id: test-all
        name: Run pytest across Python versions
        language: system
        entry: |
          uv run --python=3.10 --isolated pytest
          uv run --python=3.11 --isolated pytest
          uv run --python=3.12 --isolated pytest
          uv run --python=3.13 --isolated pytest
        shell: bash
```

- **Type**: enum
- **Default**: `null`
- **Supported values**: `sh`, `bash`, `pwsh`, `powershell`, `cmd`

When `shell` is omitted or null, `prek` preserves the current behavior: parse
`entry` into argv and invoke the command directly without a shell.

When `shell` is set, `entry` is treated as source for that shell, not as an argv
command line.

## Execution Model

The implementation should model `entry` and `shell` together as a hook entry
abstraction. That abstraction is responsible for converting user configuration
into a concrete process invocation.

It should not own unrelated execution concerns such as batching, reporter
progress, pty output, hook environment variables, working directory, or
language-specific environment setup.

### No Shell

For the default no-shell path:

1. Split `entry` with the existing argv parser.
2. Resolve the executable and shebang as today.
3. Append hook `args`.
4. Append matching filenames when `pass_filenames` allows them.

This is the compatibility path and must remain unchanged.

### Shell

For shell execution:

1. Treat `entry` as shell source.
2. Write the source to a temporary script file.
3. Build a shell-specific command invocation for that script file.
4. Pass hook `args` and matching filenames as script arguments.

Using a temporary script file avoids quoting and escaping pitfalls from placing
arbitrary multiline source behind `-c` or equivalent command-string flags.

## Shell Adapters

`shell` is a predefined adapter, not a free-form executable string. This keeps
schema validation, documentation, and cross-platform behavior precise.

### POSIX Shells

For `shell: bash`, use a non-interactive bash adapter:

```text
bash --noprofile --norc -eo pipefail <script> <args...> <filenames...>
```

For `shell: sh`, use:

```text
sh -e <script> <args...> <filenames...>
```

Inside the script, hook `args` and filenames are available through `"$@"`.

### PowerShell

For `shell: pwsh`, use PowerShell Core:

```text
pwsh -NoProfile -NonInteractive -File <script> <args...> <filenames...>
```

For `shell: powershell`, use Windows PowerShell:

```text
powershell -NoProfile -NonInteractive -File <script> <args...> <filenames...>
```

On Windows, both adapters also pass `-ExecutionPolicy Bypass`. Both adapters use
a `.ps1` temporary script. Hook `args` and filenames are available through
`$args`.

### Windows `cmd`

For `shell: cmd`, use:

```text
cmd /D /E:ON /V:OFF /S /C CALL <script> <args...> <filenames...>
```

The adapter uses a `.cmd` temporary script. Hook `args` and filenames are
available through `%*`.

## Language Interaction

Supported language backends already share the same shape:

1. Resolve the hook entry.
2. Create a process.
3. Apply language-specific PATH or environment setup.
4. Append `args` and filenames.

The new hook entry abstraction should provide a single way to build the concrete
argv for a batch, so managed and unmanaged language backends can opt into shell
execution consistently.

`language: script` needs one special rule: when `shell` is unset, the first
`entry` token remains a repository-relative script path, matching existing
behavior. When `shell` is set, `entry` is shell source and no repository-relative
script-path rewriting occurs.

Container-oriented languages may still need backend-specific handling because
they construct a command for a container runtime rather than directly executing
the hook command on the host.

??? note "Unsupported languages"

    Backends that still treat `entry` as language-specific data or parse it
    outside the shell-aware resolver should reject `shell` during validation
    instead of silently ignoring it.

    | Language | Why `shell` is unsupported |
    | -- | -- |
    | `docker`, `docker_image` | `entry` participates in container image or entrypoint selection instead of direct host process execution. |
    | `dart` | Dart package config injection requires `entry` to resolve to a direct `dart` command. |
    | `fail` | `entry` is the failure message body. |
    | `julia`, `rust` | `entry` participates in install/runtime package resolution and is split before execution. |
    | `pygrep` | `entry` is the regex pattern. |
    | `conda`, `coursier`, `perl`, `r` | The language backend is not implemented yet. |

    Predefined `repo: meta` and `repo: builtin` hooks should reject `shell` as
    well, because their entries are owned by prek.

## Design Considerations

### Explicitness

`prek` should not infer shell execution from multiline `entry` values. Formatting
an `entry` as a YAML block scalar should not change its execution semantics.

### No Ambient Shell Default

`prek` should not use the user's ambient `$SHELL` as a default. Login shells,
GUI Git clients, CI images, and shell startup files differ too much for hook
execution to be reproducible.

### Enum Instead of String

Using a `Shell` enum is intentionally less flexible than accepting arbitrary
strings such as `shell: "bash -e"`. The tradeoff is worthwhile for the initial
feature:

- JSON Schema can validate values and provide completions.
- Runtime behavior is documented per adapter.
- Windows behavior is explicit instead of guessed.
- There is no second shell-like parsing layer for the `shell` option itself.

If custom shells become necessary later, the enum can be extended with a
structured form such as:

```yaml
shell:
  command: zsh
  args: [-e]
```

That extension can be added without breaking the simple enum values.

### Fail-Fast Defaults

The predefined adapters may include fail-fast flags, such as `bash -e` or
`sh -e`, because `shell: bash` means "use prek's bash adapter" rather than "run
the literal executable `bash` with no policy." These templates must be documented
so users can reason about differences from manually running a shell.

## Compatibility

The default value is null, so existing hooks keep their current no-shell
behavior. Hooks that need shell features must opt in with `shell`.

This avoids the main compatibility risk of newline-based inline script
detection: changing YAML formatting or file existence should not change the
meaning of a hook entry.
