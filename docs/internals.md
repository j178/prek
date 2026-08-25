# Internals

This page documents some of prek's internal implementation details.

## Hook entry resolution

`repo` and `language` control different parts of hook execution:

- `repo` determines where the hook source comes from.
- `language` determines how the hook is installed and what `entry` means.

The hook checkout does not become the working directory. Hooks run against the
project being checked, including hooks loaded from a remote repository.

| Configuration | Hook source | Working directory |
| -- | -- | -- |
| `repo: local` | The project itself; there is no separate hook checkout. | The project directory. |
| `repo: <URL or path>` | A checkout cached by prek at the configured `rev`. | The project that uses the hook. |

### Direct command entries

For command-style languages such as `system`, `mise`, `python`, and `node`, prek
splits `entry` into arguments and invokes it directly. A shell is not involved:
shell operators such as `|`, `&&`, `$VAR`, and `*` are not interpreted. Use the
prek-only [`shell`](reference/configuration.md#shell) option when shell syntax is
required.

A bare command such as `ruff` is looked up on the `PATH` prepared by the language.
An explicit relative path such as `./scripts/check.sh` is resolved from the hook's
working directory. For a remote command-style hook, that means the end user's
project, not the cached hook checkout. For `repo: local`, it naturally refers to
the project that defines the hook.

Hook `args` are appended after the arguments from `entry`, followed by matching
filenames when `pass_filenames` allows them. See
[Passing arguments to hooks](authoring-hooks.md#passing-arguments-to-hooks) for
an example.

### Language-specific meanings

Most languages use the direct command rules above, with their installed tools
available on `PATH`. When `shell` is omitted, some backends intentionally give
`entry` a more specific meaning:

| Language | Meaning of `entry` |
| -- | -- |
| [`script`](languages.md#script) | The first argument is a script path. It is relative to the hook checkout for remote hooks and the project directory for local hooks. |
| [`julia`](languages.md#julia) | A Julia source path, relative to the hook checkout for remote hooks and the project directory for local hooks. |
| [`r`](languages.md#r) | `Rscript -e <expr>` or `Rscript <file>`; file paths use the same remote-checkout/local-project rule. |
| [`rust`](languages.md#rust) | The executable name; for a remote hook, it identifies a binary built and installed from the hook package. |
| [`docker`](languages.md#docker), [`docker_image`](languages.md#docker_image) | Container entrypoint, image, and argument information. The project is mounted as `/src` inside the container. |
| [`fail`](languages.md#fail), [`pygrep`](languages.md#pygrep) | A failure message or regular expression, not a command. |

The linked language sections describe the complete backend-specific behavior.

### Referencing files from the hook repository

`language: script` already resolves its script path from the remote hook checkout:

```yaml
- id: check
  name: Check files
  language: script
  entry: scripts/check.sh
```

For a command-style language, use the prek-only `{hook_repo}` placeholder when
the command or one of its arguments must refer to a file shipped in the hook
repository:

```yaml
- id: helm-lint
  name: Helm lint
  language: mise
  additional_dependencies: ["helm@4", "yq@4"]
  entry: "{hook_repo}/scripts/lint-helm-charts.sh"
```

```yaml
- id: check-config
  name: Check config
  language: system
  entry: tool --config "{hook_repo}/config/default.toml"
```

The placeholder is expanded independently inside each `entry` argument, after
command-line splitting. Quote an `entry` that begins with `{hook_repo}` so YAML
treats it as a string. For remote hooks it expands to the cached checkout; for
local hooks it expands to the project directory. It does not change the working
directory. Expansion applies only to direct command entries and `language: script`, not to `args`, filenames, `shell` source, or other backend-specific
entry forms.

!!! warning "pre-commit compatibility"

    Upstream `pre-commit` passes the placeholder through literally. Published hooks
    that depend on it should declare an appropriate `minimum_prek_version`.
