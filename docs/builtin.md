# Built-in Fast Hooks

prek includes fast, Rust-native implementations of popular hooks for speed and low overhead. These hooks are bundled directly into the `prek` binary, eliminating the need for external interpreters like Python for these specific checks.

Built-in hooks come into play in two ways:

1. **Automatic Fast Path**: Automatically replacing execution for known remote repositories.
2. **Explicit Builtin Repository**: Using `repo: builtin` for offline, zero-setup hooks.

|  | Automatic fast path | `repo: builtin` |
| -- | -- | -- |
| Config remains usable by upstream `pre-commit` | Yes | No |
| Remote repository and manifest | Cloned at the pinned `rev` | Not used |
| Environment available for fallback | Yes | Not needed |
| Network needed for first preparation | Yes | No |
| How to opt out | Set the hook's declared language or `PREK_NO_FAST_PATH=1` | Replace `repo: builtin` with a remote or local hook |

!!! note "Check implementation notes when behavior matters"

    The Rust implementations target the same purpose as their upstream hooks,
    but a hook can have documented differences in arguments, defaults, or edge
    cases. Check its entry in the [Hook Reference](#hook-reference). To compare
    behavior, disable the fast path and run the pinned implementation.

## 1. Automatic Fast Path

When you use a standard configuration pointing to a supported repository (like `https://github.com/pre-commit/pre-commit-hooks`), `prek` automatically detects this and runs its internal Rust implementation instead of the Python version defined in the repository.

The fast path is activated when the `repo` URL matches `https://github.com/pre-commit/pre-commit-hooks`. No need to change anything in your configuration.
The `rev` field does not affect fast-path detection. It still selects the
manifest that prek reads and the repository implementation used for fallback.

This provides a speed boost while keeping your configuration compatible with the original `pre-commit` tool.

```yaml
repos:
  - repo: https://github.com/pre-commit/pre-commit-hooks  # Enables fast path
    rev: v4.5.0  # Used for the manifest and fallback, not fast-path detection
    hooks:
      - id: trailing-whitespace
```

!!! note

    In this mode, `prek` will still clone the repository and create the environment (e.g., a Python venv) to ensure full compatibility and fallback capabilities. However, the actual hook execution bypasses the environment and runs the native Rust code.

### Supported Hooks

Currently, only part of hooks from `https://github.com/pre-commit/pre-commit-hooks` is supported. More popular repositories may be added over time.

### <https://github.com/pre-commit/pre-commit-hooks>

- [`trailing-whitespace`](https://github.com/pre-commit/pre-commit-hooks#trailing-whitespace) (Trims trailing whitespace.)
- [`check-added-large-files`](https://github.com/pre-commit/pre-commit-hooks#check-added-large-files) (Prevents giant files from being committed.)
- [`check-case-conflict`](https://github.com/pre-commit/pre-commit-hooks#check-case-conflict) (Checks for files that would conflict in case-insensitive filesystems.)
- [`check-illegal-windows-names`](https://github.com/pre-commit/pre-commit-hooks#check-illegal-windows-names) (Checks for filenames which cannot be created on Windows.)
- [`end-of-file-fixer`](https://github.com/pre-commit/pre-commit-hooks#end-of-file-fixer) (Ensures that a file is either empty, or ends with one newline.)
- [`file-contents-sorter`](https://github.com/pre-commit/pre-commit-hooks#file-contents-sorter) (Sorts the lines in specified files (defaults to alphabetical).)
- [`requirements-txt-fixer`](https://github.com/pre-commit/pre-commit-hooks#requirements-txt-fixer) (Sorts entries in requirements.txt.)
- [`fix-byte-order-marker`](https://github.com/pre-commit/pre-commit-hooks#fix-byte-order-marker) (Removes UTF-8 byte order marker.)
- [`forbid-new-submodules`](https://github.com/pre-commit/pre-commit-hooks#forbid-new-submodules) (Prevents the addition of new Git submodules.)
- [`check-json`](https://github.com/pre-commit/pre-commit-hooks#check-json) (Checks JSON files for parseable syntax.)
- [`check-toml`](https://github.com/pre-commit/pre-commit-hooks#check-toml) (Checks TOML files for parseable syntax.)
- [`check-vcs-permalinks`](https://github.com/pre-commit/pre-commit-hooks#check-vcs-permalinks) (Ensures that links to VCS websites are permalinks.)
- [`check-yaml`](https://github.com/pre-commit/pre-commit-hooks#check-yaml) (Checks YAML files for parseable syntax.)
- [`check-xml`](https://github.com/pre-commit/pre-commit-hooks#check-xml) (Checks XML files for parseable syntax.)
- [`mixed-line-ending`](https://github.com/pre-commit/pre-commit-hooks#mixed-line-ending) (Replaces or checks mixed line endings.)
- [`check-symlinks`](https://github.com/pre-commit/pre-commit-hooks#check-symlinks) (Checks for symlinks which do not point to anything.)
- [`destroyed-symlinks`](https://github.com/pre-commit/pre-commit-hooks#destroyed-symlinks) (Detects symlinks that were replaced with regular files whose contents are the original symlink target path.)
- [`check-merge-conflict`](https://github.com/pre-commit/pre-commit-hooks#check-merge-conflict) (Checks for files that contain merge conflict strings.)
- [`detect-private-key`](https://github.com/pre-commit/pre-commit-hooks#detect-private-key) (Detects the presence of private keys.)
- [`no-commit-to-branch`](https://github.com/pre-commit/pre-commit-hooks#no-commit-to-branch) (Protects specific branches from direct commits.)
- [`check-shebang-scripts-are-executable`](https://github.com/pre-commit/pre-commit-hooks#check-shebang-scripts-are-executable) (Ensures that (non-binary) files with a shebang are executable.)
- [`check-executables-have-shebangs`](https://github.com/pre-commit/pre-commit-hooks#check-executables-have-shebangs) (Ensures that (non-binary) executables have a shebang.)

#### Notes

- `pretty-format-json` is currently available only via `repo: builtin` while parity coverage against upstream Python behavior is still being expanded.
- Other hooks from the repository which have no fast path implementation will run via the standard method.

### Disabling the fast path

To use the pinned repository implementation for a single hook, explicitly set the language
declared by that hook:

```yaml
repos:
  - repo: https://github.com/pre-commit/pre-commit-hooks
    rev: v6.0.0
    hooks:
      - id: check-yaml
        language: python  # Use the pinned repository implementation
```

To disable the fast path for every hook in a prek invocation:

```bash
PREK_NO_FAST_PATH=1 prek run
```

This forces prek to fall back to the standard execution path.

## 2. Explicit Builtin Repository

You can explicitly tell `prek` to use its internal hooks by setting `repo: builtin`.

This mode has significant benefits:

- **No network required**: Does not clone any repository.
- **No environment setup**: Does not create Python environments or install dependencies.
- **Maximum speed**: Instant startup and execution.

**Note**: Configurations using `repo: builtin` are **not compatible** with the standard `pre-commit` tool.

=== "prek.toml"

    ```toml
    [[repos]]
    repo = "builtin"
    hooks = [
      { id = "trailing-whitespace" },
      { id = "check-added-large-files" },
    ]
    ```

=== ".pre-commit-config.yaml"

    ```yaml
    repos:
      - repo: builtin
        hooks:
          - id: trailing-whitespace
          - id: check-added-large-files
    ```

List the builtins bundled with your installed prek version using:

```bash
prek util list-builtins -v
```

### Supported Hooks

For `repo: builtin`, the following hooks are supported:

- [`trailing-whitespace`](#trailing-whitespace) (Trims trailing whitespace.)
- [`check-added-large-files`](#check-added-large-files) (Prevents giant files from being committed.)
- [`check-case-conflict`](#check-case-conflict) (Checks for files that would conflict in case-insensitive filesystems.)
- [`check-illegal-windows-names`](#check-illegal-windows-names) (Checks for filenames which cannot be created on Windows.)
- [`end-of-file-fixer`](#end-of-file-fixer) (Ensures that a file is either empty, or ends with one newline.)
- [`file-contents-sorter`](#file-contents-sorter) (Sorts the lines in specified files (defaults to alphabetical).)
- [`requirements-txt-fixer`](#requirements-txt-fixer) (Sorts entries in requirements.txt.)
- [`fix-byte-order-marker`](#fix-byte-order-marker) (Removes UTF-8 byte order marker.)
- [`forbid-new-submodules`](#forbid-new-submodules) (Prevents the addition of new Git submodules.)
- [`check-json`](#check-json) (Checks JSON files for parseable syntax.)
- [`check-json5`](#check-json5) (Checks JSON5 files for parseable syntax.)
- [`pretty-format-json`](#pretty-format-json) (Checks that JSON files are pretty-formatted.)
- [`check-toml`](#check-toml) (Checks TOML files for parseable syntax.)
- [`check-vcs-permalinks`](#check-vcs-permalinks) (Ensures that links to VCS websites are permalinks.)
- [`check-yaml`](#check-yaml) (Checks YAML files for parseable syntax.)
- [`check-xml`](#check-xml) (Checks XML files for parseable syntax.)
- [`deny-filename-pattern`](#deny-filename-pattern) (Fails if any selected filename matches a regular expression.)
- [`deny-pattern`](#deny-pattern) (Fails if any file contains a matching regular expression.)
- [`require-filename-pattern`](#require-filename-pattern) (Fails if any selected filename does not match a regular expression.)
- [`require-pattern`](#require-pattern) (Fails if any file does not contain a matching regular expression.)
- [`mixed-line-ending`](#mixed-line-ending) (Replaces or checks mixed line endings.)
- [`check-symlinks`](#check-symlinks) (Checks for symlinks which do not point to anything.)
- [`destroyed-symlinks`](#destroyed-symlinks) (Detects symlinks that were replaced with regular files whose contents are the original symlink target path.)
- [`check-merge-conflict`](#check-merge-conflict) (Checks for files that contain merge conflict strings.)
- [`detect-private-key`](#detect-private-key) (Detects the presence of private keys.)
- [`no-commit-to-branch`](#no-commit-to-branch) (Protects specific branches from direct commits.)
- [`check-shebang-scripts-are-executable`](#check-shebang-scripts-are-executable) (Ensures that (non-binary) files with a shebang are executable.)
- [`check-executables-have-shebangs`](#check-executables-have-shebangs) (Ensures that (non-binary) executables have a shebang.)

### Hook Reference

This section documents the built-in (Rust) implementations used by `repo: builtin`.

#### Configuration notes

- Configure arguments via `args: [...]` just like `pre-commit`.
- For `repo: builtin`, `entry` is not allowed and `language` must be `system` (it is fine to omit `language`).
- Some hooks are **fixers** (they modify files). Like `pre-commit-hooks`, they typically exit non-zero after making changes so you can re-run the commit.

Example:

```yaml
repos:
  - repo: builtin
    hooks:
      - id: trailing-whitespace
        args: [--markdown-linebreak-ext=md]
      - id: check-added-large-files
        args: [--maxkb=1024]
```

---

#### `trailing-whitespace`

Trims trailing whitespace from each line.

**Supported arguments** (compatible with `pre-commit-hooks`):

- `--markdown-linebreak-ext=<ext>` (repeatable / comma-separated)
    - Preserves Markdown hard line breaks (two trailing spaces) for files with the given extension(s).
    - Use `--markdown-linebreak-ext=*` to treat **all** files as Markdown.
- `--chars=<chars>`
    - Trim only the specified set of characters instead of “all trailing whitespace”.
    - Example: `args: [--chars, " \t"]` (space + tab).

**Caveats**

- `--markdown-linebreak-ext` values must be extensions only (no path separators).

---

#### `check-added-large-files`

Prevents giant files from being committed.

**Supported arguments** (compatible with `pre-commit-hooks`):

- `--maxkb=<N>` (default: `500`)
    - Maximum allowed file size, in kibibytes.
- `--enforce-all`
    - Check all matched files, not just those staged for addition.

**Caveats**

- By default, only files staged for **addition** are checked.
- Files configured with `filter=lfs` (via git attributes) are skipped.

---

#### `check-case-conflict`

Checks for paths that would conflict on a case-insensitive filesystem (for example macOS / Windows).

**Supported arguments**

- None.

**Caveats**

- The check includes parent directories as well as file paths, to catch directory-level case conflicts.

---

#### `check-illegal-windows-names`

Checks for filenames that cannot be created on Windows.

**Supported arguments**

- None.

**Behavior / caveats**

- Reports filenames containing Windows-reserved device names such as `CON`, `PRN`, `AUX`, `NUL`, `COM1`, and `LPT1`.
- Reports filenames containing characters forbidden by Windows, including `<`, `>`, `:`, `"`, `\`, `|`, `?`, `*`, and control characters.
- Reports path segments ending with a trailing `.` or space.

---

#### `end-of-file-fixer`

Ensures files end in a newline and only a newline.

**Supported arguments**

- None.

**Behavior / caveats**

- Empty files are left unchanged.
- Files containing only newlines are truncated to empty.
- If a file has no trailing newline, a single `\n` is appended (even if the file otherwise uses CRLF).
- If a file has trailing newlines, they are reduced to exactly one trailing line ending.

---

#### `file-contents-sorter`

Sorts the non-empty lines in each matched file and rewrites the file when the normalized order changes.

**Supported arguments** (compatible with `pre-commit-hooks`):

- `--ignore-case`
    - Sort using ASCII case-folded ordering.
    - Mutually exclusive with `--unique`.
- `--unique`
    - Sort and deduplicate lines.
    - Mutually exclusive with `--ignore-case`.

**Behavior / caveats**

- Blank lines and whitespace-only lines are removed before sorting.
- Line endings are normalized to `\n` in the rewritten file.
- Like upstream, the builtin hook defaults to `files: '^$'`, so you must configure `files:` explicitly to target specific files.

Example:

```yaml
repos:
  - repo: builtin
    hooks:
      - id: file-contents-sorter
        files: ^requirements(-dev)?\.txt$
```

---

#### `requirements-txt-fixer`

Sorts entries in Python `requirements*.txt` and `constraints*.txt` files by their case-insensitive requirement name.

**Behavior / caveats**

- The default file pattern is `(requirements|constraints).*\.txt$`.
- Leading comments and continuation lines stay attached to their requirement while sorting. Top-of-file and trailing comment blocks are preserved.
- Exact duplicate entries are collapsed, preferring the copy with an attached comment.
- Exact `pkg-resources==0.0.0` and `pkg_resources==0.0.0` entries are removed, matching upstream.
- This is a sorter, not a full PEP 508 validator. It uses the same lightweight name extraction as `pre-commit-hooks`.

---

#### `fix-byte-order-marker`

Removes a UTF-8 byte order marker (BOM) from the beginning of a file.

**Supported arguments**

- None.

**Caveats**

- Only removes the UTF-8 BOM (`EF BB BF`).

---

#### `forbid-new-submodules`

Prevents the addition of new Git submodules.

**Supported arguments**

- None.

**Behavior / caveats**

- Existing submodules are allowed; only submodules newly added by the checked changes are reported.
- Staged changes are checked by default. When `PRE_COMMIT_FROM_REF` and `PRE_COMMIT_TO_REF` are both set, their revision range is checked instead.

---

#### `check-json`

Attempts to load all JSON files to verify syntax.

**Supported arguments**

- None.

**Caveats / differences**

- This implementation rejects **duplicate object keys** (errors with `duplicate key ...`).
- The parser disables the default recursion limit and uses a stack-friendly drop strategy for deeply nested JSON.

---

#### `check-json5`

Attempts to load all JSON5 files to verify syntax.

**Supported arguments**

- None.

**Caveats / differences**

- This implementation rejects **duplicate object keys** (errors with `duplicate key ...`).

---

#### `pretty-format-json`

Checks that JSON files are pretty-formatted and can optionally rewrite them in place.

**Supported arguments** (compatible with `pre-commit-hooks`):

- `--autofix`
    - Rewrite files in place when formatting changes are needed.
- `--indent=<indent>` (default: `2`)
    - Use `<indent>` for each indentation level.
    - Numeric values mean that many spaces.
    - Non-numeric values are used literally, so `--indent=\t` uses tabs.
- `--no-ensure-ascii`
    - Keep non-ASCII characters as UTF-8 instead of escaping them as `\uXXXX`.
- `--no-sort-keys`
    - Preserve the original key order instead of sorting object keys.
- `--top-keys=<k1,k2,...>`
    - In every JSON object, move matching keys to the front in the given order.
    - Duplicate names after the first one are ignored.
    - Remaining keys come after that prefix and are sorted unless `--no-sort-keys` is set.
    - This applies recursively to nested objects too, not just the root object.

**Caveats**

- This hook is currently available only via `repo: builtin`; automatic fast-path replacement of the upstream Python hook remains disabled until parity coverage is broader.
- Rewritten files always use LF (`\n`) line endings and end with exactly one trailing newline.

---

#### `check-toml`

Attempts to load all TOML files to verify syntax.

**Supported arguments**

- None.

**Caveats**

- Files must be valid UTF-8; invalid UTF-8 is reported as an error.
- May report multiple parse errors for a single file.

---

#### `check-vcs-permalinks`

Ensures that links to VCS websites are permalinks.

**Supported arguments** (compatible with `pre-commit-hooks`):

- `--additional-github-domain=<domain>` (repeatable)
    - Adds extra GitHub-style domains to check in addition to the default `github.com`.

**Behavior / caveats**

- Flags links of the form `https://<domain>/<owner>/<repo>/blob/<branch>/...#L...`.
- Does not flag commit-hash permalinks where `<branch>` is already a 4-64 character hexadecimal revision.
- The builtin and fast-path implementations currently follow the upstream hook's GitHub-family matching behavior.

---

#### `check-yaml`

Attempts to load all YAML files to verify syntax.

**Supported arguments** (partially compatible with `pre-commit-hooks`):

- `-m`, `--allow-multiple-documents` (alias: `--multi`)
    - Allow YAML multi-document syntax (`---`).
- `--unsafe`
    - Parse YAML syntax without loading it. Implies `--allow-multiple-documents`.

---

#### `check-xml`

Attempts to load all XML files to verify syntax.

**Supported arguments**

- None.

**Caveats**

- Empty files are treated as invalid XML.
- Fails if there is “junk after the document element” (multiple top-level roots).

---

#### `deny-filename-pattern`

Fails when the final path component (the basename) of any selected file matches a configured regular expression. Patterns use the [Rust `regex` syntax](https://docs.rs/regex/latest/regex/#syntax). When multiple patterns are provided, the hook fails when a basename matches any one of them.

The standard `files`, `exclude`, and type filters select which project-relative paths are checked. The patterns passed to this hook are then matched only against each selected basename.

**Supported arguments**

- `PATTERN...` (required)
    - Positional regular expressions to deny.
    - Use `--` before a pattern that begins with `-`.
- `-i`, `--ignore-case`
    - Match all patterns case-insensitively.

Each matching file is reported once as `path: filename matches a denied pattern`.

```yaml
repos:
  - repo: builtin
    hooks:
      - id: deny-filename-pattern
        name: disallow spaces in filenames
        args: ['\s']
```

---

#### `deny-pattern`

Fails when any selected text file matches a configured regular expression.
Patterns use the [Rust `regex` syntax](https://docs.rs/regex/latest/regex/#syntax).
When multiple patterns are provided, matching any one of them is sufficient.

**Supported arguments**

- `PATTERN...` (required)
    - Positional regular expressions to deny.
    - Use `--` before a pattern that begins with `-`.
- `-i`, `--ignore-case`
    - Match all patterns case-insensitively.
- `-m`, `--multiline`
    - Search each file as a whole, with `^` and `$` matching line boundaries and `.` matching newlines.
    - Reads each selected file into memory.

By default, each matching line is reported as `path:line:contents`. A line matching more than one pattern is reported only once. With `--multiline`, the earliest match in each file is reported as `path:start-line:matched-block`.

```yaml
repos:
  - repo: builtin
    hooks:
      - id: deny-pattern
        name: disallow wildcard imports
        args: ['^\s*#import\s+.+:\s*\*']
        files: \.typ$
```

---

#### `require-filename-pattern`

Fails when the final path component (the basename) of any selected file does not match at least one configured regular expression. This is a per-file requirement: every selected basename must match, while different basenames may match different patterns.

`require-filename-pattern` supports the same positional `PATTERN...` and `-i` / `--ignore-case` arguments as [`deny-filename-pattern`](#deny-filename-pattern). Matching uses search semantics; use `^` and `$` when the pattern must match the entire basename. Files without a match are reported as `path: filename does not match any required pattern`.

```yaml
repos:
  - repo: builtin
    hooks:
      - id: require-filename-pattern
        name: python tests naming
        args:
          - '^test_.*\.py$'
          - '^__init__\.py$'
          - '^conftest\.py$'
        files: '(^|/)tests/.+\.py$'
```

---

#### `require-pattern`

Fails when any selected text file does not match at least one configured regular expression. This is a per-file requirement: every file must match, while different files may match different patterns.

`require-pattern` supports the same positional `PATTERN...`, `-i` / `--ignore-case`, and `--multiline` arguments as [`deny-pattern`](#deny-pattern). Files without a match are reported as `path: no pattern matched`.

```yaml
repos:
  - repo: builtin
    hooks:
      - id: require-pattern
        name: require a copyright notice
        args: [--ignore-case, copyright]
        files: '\.(rs|py)$'
```

---

#### `mixed-line-ending`

Replaces or checks mixed line endings.

**Supported arguments** (compatible with `pre-commit-hooks`, plus one extra mode):

- `--fix=<mode>` (default: `auto`)
    - `auto`: replace with the most frequent line ending in the file.
    - `no`: check only (do not modify files).
    - `lf`: convert to LF (`\n`).
    - `crlf`: convert to CRLF (`\r\n`).
    - `cr`: convert to CR (`\r`) (extra mode in `prek`).

**Caveats**

- Empty and binary files (containing NUL) are skipped.
- Upstream note: forcing `lf` / `crlf` may not behave as expected with git CRLF conversion settings (for example `core.autocrlf`).

---

#### `check-symlinks`

Checks for symlinks which do not point to anything.

**Supported arguments**

- None.

**Caveats**

- Relies on filesystem symlink support. On Windows, symlink creation and detection can be permission-dependent.

---

#### `destroyed-symlinks`

Detects files staged as regular files whose `HEAD` version is a symlink, which usually happens when a repository is checked out in an environment without symlink support.

**Supported arguments**

- None.

**Caveats**

- This matches upstream `pre-commit-hooks` behavior: it only checks tracked entries reported by `git status --porcelain=v2`.
- It intentionally ignores differences consisting only of trailing ASCII whitespace (including spaces, tabs, and newline/CRLF conversions) when comparing the staged file against the original symlink target path, because those differences are commonly introduced by formatting hooks.

---

#### `check-merge-conflict`

Checks for merge conflict markers.

**Supported arguments** (compatible with `pre-commit-hooks`):

- `--assume-in-merge`
    - Allow running the hook even when there is no merge/rebase state detected.

**Caveats**

- By default, this hook exits successfully when not in a merge/rebase state.
- Detects conflict markers only when they appear at the start of a line.
- Detects standard conflict blocks (`<<<<<<<`, `=======`, `>>>>>>>`) and diff3 ancestor markers (`|||||||`).
- `=======` is only reported after a preceding `<<<<<<<`, which avoids false positives for content such as reStructuredText headings.

---

#### `detect-private-key`

Detects the presence of private keys.

**Supported arguments**

- None.

**Caveats**

- This is a heuristic substring scan for common PEM/key headers (e.g. `BEGIN RSA PRIVATE KEY`, `BEGIN OPENSSH PRIVATE KEY`, `BEGIN PGP PRIVATE KEY BLOCK`, etc.).
  It can produce false positives/negatives.

---

#### `no-commit-to-branch`

Protects specific branches from direct commits.

**Supported arguments** (compatible with `pre-commit-hooks`):

- `-b`, `--branch <branch>` (repeatable, default: `main`, `master`)
- `-p`, `--pattern <regex>` (repeatable)

**Caveats**

- This hook is configured as `always_run: true` by default, and does not take filenames.
  As a result, `files`, `exclude`, `types`, etc. are ignored unless you explicitly set `always_run: false`.
- If HEAD is detached (no current branch), the hook does nothing.

---

#### `check-executables-have-shebangs`

Checks that non-binary executables have a proper shebang.

**Supported arguments**

- None.

**Caveats**

- The check is intentionally lightweight: it only verifies that the file starts with `#!`.
- On systems where the executable bit is not tracked by the filesystem, `prek` consults git’s staged mode bits.

---

#### `check-shebang-scripts-are-executable`

Checks that non-binary files with a shebang are marked executable.

**Supported arguments**

- None.

**Caveats**

- The check is intentionally lightweight: it only verifies that the file starts with `#!`.
- To work on filesystems which do not track the executable bit, `prek` consults git’s staged mode bits.
