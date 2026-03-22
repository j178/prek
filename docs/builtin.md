# Built-in Fast Hooks

prek includes fast, Rust-native implementations of popular hooks for speed and low overhead. These hooks are bundled directly into the `prek` binary, eliminating the need for external interpreters like Python for these specific checks.

Built-in hooks come into play in two ways:

1. **Automatic Fast Path**: Automatically replacing execution for known remote repositories.
2. **Explicit Builtin Repository**: Using `repo: builtin` for offline, zero-setup hooks.

## 1. Automatic Fast Path

When you use a standard configuration pointing to a supported repository (like `https://github.com/pre-commit/pre-commit-hooks`), `prek` automatically detects this and runs its internal Rust implementation instead of the Python version defined in the repository.

The fast path is activated when the `repo` URL matches `https://github.com/pre-commit/pre-commit-hooks`. No need to change anything in your configuration.
Note that the `rev` field is ignored for detection purposes.

This provides a speed boost while keeping your configuration compatible with the original `pre-commit` tool.

```yaml
repos:
  - repo: https://github.com/pre-commit/pre-commit-hooks  # Enables fast path
    rev: v4.5.0  # This is ignored for fast path detection
    hooks:
      - id: trailing-whitespace
```

!!! note

    In this mode, `prek` will still clone the repository and create the environment (e.g., a Python venv) to ensure full compatibility and fallback capabilities. However, the actual hook execution bypasses the environment and runs the native Rust code.

### Supported Hooks

Currently, only part of hooks from `https://github.com/pre-commit/pre-commit-hooks` is supported. More popular repositories may be added over time.

### <https://github.com/pre-commit/pre-commit-hooks>

- [`trailing-whitespace`](https://github.com/pre-commit/pre-commit-hooks#trailing-whitespace) (Trim trailing whitespace)
- [`check-added-large-files`](https://github.com/pre-commit/pre-commit-hooks#check-added-large-files) (Prevent committing large files)
- [`check-case-conflict`](https://github.com/pre-commit/pre-commit-hooks#check-case-conflict) (Check for files that would conflict in case-insensitive filesystems)
- [`end-of-file-fixer`](https://github.com/pre-commit/pre-commit-hooks#end-of-file-fixer) (Ensure newline at EOF)
- [`file-contents-sorter`](https://github.com/pre-commit/pre-commit-hooks#file-contents-sorter) (Sort lines in explicitly targeted files)
- [`fix-byte-order-marker`](https://github.com/pre-commit/pre-commit-hooks#fix-byte-order-marker) (Remove UTF-8 byte order marker)
- [`forbid-new-submodules`](https://github.com/pre-commit/pre-commit-hooks#forbid-new-submodules) (Prevent addition of new git submodules.)
- [`check-json`](https://github.com/pre-commit/pre-commit-hooks#check-json) (Validate JSON files)
- [`check-toml`](https://github.com/pre-commit/pre-commit-hooks#check-toml) (Validate TOML files)
- [`check-vcs-permalinks`](https://github.com/pre-commit/pre-commit-hooks#check-vcs-permalinks) (Check that VCS links are permalinks)
- [`check-yaml`](https://github.com/pre-commit/pre-commit-hooks#check-yaml) (Validate YAML files)
- [`check-xml`](https://github.com/pre-commit/pre-commit-hooks#check-xml) (Validate XML files)
- [`mixed-line-ending`](https://github.com/pre-commit/pre-commit-hooks#mixed-line-ending) (Normalize or check line endings)
- [`check-symlinks`](https://github.com/pre-commit/pre-commit-hooks#check-symlinks) (Check for broken symlinks)
- [`destroyed-symlinks`](https://github.com/pre-commit/pre-commit-hooks#destroyed-symlinks) (Detect destroyed symlinks)
- [`check-merge-conflict`](https://github.com/pre-commit/pre-commit-hooks#check-merge-conflict) (Check for merge conflicts)
- [`detect-private-key`](https://github.com/pre-commit/pre-commit-hooks#detect-private-key) (Detect private keys)
- [`no-commit-to-branch`](https://github.com/pre-commit/pre-commit-hooks#no-commit-to-branch) (Prevent committing to protected branches)
- [`check-shebang-scripts-are-executable`](https://github.com/pre-commit/pre-commit-hooks#check-shebang-scripts-are-executable) (Ensures that (non-binary) files with a shebang are executable)
- [`check-executables-have-shebangs`](https://github.com/pre-commit/pre-commit-hooks#check-executables-have-shebangs) (Ensures that (non-binary) executables have a shebang)

#### Notes

- `check-yaml` fast path does not yet support the `--unsafe` flag; for those cases, the automatic fast path is skipped.
- Other hooks from the repository which have no fast path implementation will run via the standard method.

### Disabling the fast path

If you need to compare with the original behavior or encounter differences:

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

```yaml
repos:
  - repo: builtin
    hooks:
      - id: trailing-whitespace
      - id: check-added-large-files
```

### Supported Hooks

For `repo: builtin`, the following hooks are supported:

- [`trailing-whitespace`](#trailing-whitespace) (Trim trailing whitespace)
- [`check-added-large-files`](#check-added-large-files) (Prevent committing large files)
- [`check-case-conflict`](#check-case-conflict) (Check for files that would conflict in case-insensitive filesystems)
- [`check-illegal-windows-names`](#check-illegal-windows-names) (Check for filenames invalid on Windows)
- [`end-of-file-fixer`](#end-of-file-fixer) (Ensure newline at EOF)
- [`file-contents-sorter`](#file-contents-sorter) (Sort lines in explicitly targeted files)
- [`fix-byte-order-marker`](#fix-byte-order-marker) (Remove UTF-8 byte order marker)
- [`check-json`](#check-json) (Validate JSON files)
- [`check-json5`](#check-json5) (Validate JSON5 files)
- [`check-toml`](#check-toml) (Validate TOML files)
- [`check-vcs-permalinks`](#check-vcs-permalinks) (Check that VCS links are permalinks)
- [`check-yaml`](#check-yaml) (Validate YAML files)
- [`check-xml`](#check-xml) (Validate XML files)
- [`mixed-line-ending`](#mixed-line-ending) (Normalize or check line endings)
- [`check-symlinks`](#check-symlinks) (Check for broken symlinks)
- [`destroyed-symlinks`](#destroyed-symlinks) (Detect destroyed symlinks)
- [`check-merge-conflict`](#check-merge-conflict) (Check for merge conflicts)
- [`detect-private-key`](#detect-private-key) (Detect private keys)
- [`no-commit-to-branch`](#no-commit-to-branch) (Prevent committing to protected branches)
- [`check-shebang-scripts-are-executable`](#check-shebang-scripts-are-executable) (Ensures that (non-binary) files with a shebang are executable)
- [`check-executables-have-shebangs`](#check-executables-have-shebangs) (Ensures that (non-binary) executables have a shebang)

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

#### `fix-byte-order-marker`

Removes a UTF-8 byte order marker (BOM) from the beginning of a file.

**Supported arguments**

- None.

**Caveats**

- Only removes the UTF-8 BOM (`EF BB BF`).

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

**Caveats / differences**

- `--unsafe` is not supported.
    - With `repo: builtin`, passing `--unsafe` is treated as an unknown argument.

---

#### `check-xml`

Attempts to load all XML files to verify syntax.

**Supported arguments**

- None.

**Caveats**

- Empty files are treated as invalid XML.
- Fails if there is “junk after the document element” (multiple top-level roots).

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

Checks for merge conflict strings.

**Supported arguments** (compatible with `pre-commit-hooks`):

- `--assume-in-merge`
    - Allow running the hook even when there is no merge/rebase state detected.

**Caveats**

- By default, this hook exits successfully when not in a merge/rebase state.
- Detects common conflict markers only when they appear at the start of a line.

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
