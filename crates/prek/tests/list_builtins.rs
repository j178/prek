use crate::common::{TestContext, cmd_snapshot};

mod common;

#[test]
fn list_builtins_basic() {
    let context = TestContext::new();

    cmd_snapshot!(context.filters(), context.command().arg("util").arg("list-builtins"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    check-added-large-files
    check-case-conflict
    check-executables-have-shebangs
    check-illegal-windows-names
    check-json
    check-json5
    check-merge-conflict
    check-shebang-scripts-are-executable
    check-symlinks
    check-toml
    check-vcs-permalinks
    check-xml
    check-yaml
    deny-filename-pattern
    deny-pattern
    destroyed-symlinks
    detect-private-key
    end-of-file-fixer
    file-contents-sorter
    fix-byte-order-marker
    forbid-new-submodules
    mixed-line-ending
    no-commit-to-branch
    pretty-format-json
    require-filename-pattern
    require-pattern
    requirements-txt-fixer
    trailing-whitespace

    ----- stderr -----
    ");
}

#[test]
fn list_builtins_verbose() {
    let context = TestContext::new();

    cmd_snapshot!(context.filters(), context.command().arg("util").arg("list-builtins").arg("--verbose"), @"
    success: true
    exit_code: 0
    ----- stdout -----
    check-added-large-files
      Prevents giant files from being committed.
      flags:
            --enforce-all     Check all files, not just those staged for addition
            --maxkb <MAX_KB>  Maximum allowed file size in KiB [default: 500]

    check-case-conflict
      Checks for files that would conflict in case-insensitive filesystems.

    check-executables-have-shebangs
      Ensures that (non-binary) executables have a shebang.

    check-illegal-windows-names
      Checks for filenames which cannot be created on Windows.

    check-json
      Checks JSON files for parseable syntax.

    check-json5
      Checks JSON5 files for parseable syntax.

    check-merge-conflict
      Checks for files that contain merge conflict strings.
      flags:
            --assume-in-merge  Run even when no merge or rebase is detected

    check-shebang-scripts-are-executable
      Ensures that (non-binary) files with a shebang are executable.

    check-symlinks
      Checks for symlinks which do not point to anything.

    check-toml
      Checks TOML files for parseable syntax.

    check-vcs-permalinks
      Ensures that links to VCS websites are permalinks.
      flags:
            --additional-github-domain <DOMAIN>  Additional GitHub-style domain to check (repeatable)

    check-xml
      Checks XML files for parseable syntax.

    check-yaml
      Checks YAML files for parseable syntax.
      flags:
        -m, --allow-multiple-documents  Allow multiple YAML documents [alias: --multi]

    deny-filename-pattern
      Fails if any selected filename matches a regular expression.
      flags:
        -i, --ignore-case  Match patterns case-insensitively

    deny-pattern
      Fails if any file contains a matching regular expression.
      flags:
        -i, --ignore-case  Match patterns case-insensitively
        -m, --multiline    Search each file as a whole

    destroyed-symlinks
      Detects symlinks that were replaced with regular files whose contents are the original symlink target path.

    detect-private-key
      Detects the presence of private keys.

    end-of-file-fixer
      Ensures that a file is either empty, or ends with one newline.

    file-contents-sorter
      Sorts the lines in specified files (defaults to alphabetical).
      flags:
            --ignore-case  Sort lines case-insensitively
            --unique       Remove duplicate lines

    fix-byte-order-marker
      Removes UTF-8 byte order marker.

    forbid-new-submodules
      Prevents the addition of new Git submodules.

    mixed-line-ending
      Replaces or checks mixed line endings.
      flags:
        -f, --fix <FIX>  Fix mixed line endings by converting to the most common line ending or a
                         specified line ending [default: auto] [possible values: auto, no, lf, crlf, cr]

    no-commit-to-branch
      Protects specific branches from direct commits.
      flags:
        -b, --branch <BRANCH>  Branch to protect (repeatable) [default: main master]
        -p, --pattern <REGEX>  Regular expression matching branches to protect (repeatable)

    pretty-format-json
      Checks that JSON files are pretty-formatted.
      flags:
            --autofix          Rewrite files in place
            --indent <INDENT>  Indentation width or string [default: 2]
            --no-ensure-ascii  Keep non-ASCII characters as UTF-8
            --no-sort-keys     Preserve object key order
            --top-keys <KEYS>  Object keys to move to the front, comma-separated

    require-filename-pattern
      Fails if any selected filename does not match a regular expression.
      flags:
        -i, --ignore-case  Match patterns case-insensitively

    require-pattern
      Fails if any file does not contain a matching regular expression.
      flags:
        -i, --ignore-case  Match patterns case-insensitively
        -m, --multiline    Search each file as a whole

    requirements-txt-fixer
      Sorts entries in requirements.txt.

    trailing-whitespace
      Trims trailing whitespace.
      flags:
            --markdown-linebreak-ext <EXT>  Preserve Markdown hard line breaks for EXT (repeatable)
            --chars <CHARS>                 Trim only these characters


    ----- stderr -----
    ");
}

#[test]
fn list_builtins_json() {
    let context = TestContext::new();

    cmd_snapshot!(context.filters(), context.command().arg("util").arg("list-builtins").arg("--output-format=json"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    [
      {
        "id": "check-added-large-files",
        "name": "check for added large files",
        "description": "Prevents giant files from being committed."
      },
      {
        "id": "check-case-conflict",
        "name": "check for case conflicts",
        "description": "Checks for files that would conflict in case-insensitive filesystems."
      },
      {
        "id": "check-executables-have-shebangs",
        "name": "check that executables have shebangs",
        "description": "Ensures that (non-binary) executables have a shebang."
      },
      {
        "id": "check-illegal-windows-names",
        "name": "check illegal windows names",
        "description": "Checks for filenames which cannot be created on Windows."
      },
      {
        "id": "check-json",
        "name": "check json",
        "description": "Checks JSON files for parseable syntax."
      },
      {
        "id": "check-json5",
        "name": "check json5",
        "description": "Checks JSON5 files for parseable syntax."
      },
      {
        "id": "check-merge-conflict",
        "name": "check for merge conflicts",
        "description": "Checks for files that contain merge conflict strings."
      },
      {
        "id": "check-shebang-scripts-are-executable",
        "name": "check that scripts with shebangs are executable",
        "description": "Ensures that (non-binary) files with a shebang are executable."
      },
      {
        "id": "check-symlinks",
        "name": "check for broken symlinks",
        "description": "Checks for symlinks which do not point to anything."
      },
      {
        "id": "check-toml",
        "name": "check toml",
        "description": "Checks TOML files for parseable syntax."
      },
      {
        "id": "check-vcs-permalinks",
        "name": "check vcs permalinks",
        "description": "Ensures that links to VCS websites are permalinks."
      },
      {
        "id": "check-xml",
        "name": "check xml",
        "description": "Checks XML files for parseable syntax."
      },
      {
        "id": "check-yaml",
        "name": "check yaml",
        "description": "Checks YAML files for parseable syntax."
      },
      {
        "id": "deny-filename-pattern",
        "name": "deny filename patterns",
        "description": "Fails if any selected filename matches a regular expression."
      },
      {
        "id": "deny-pattern",
        "name": "deny patterns",
        "description": "Fails if any file contains a matching regular expression."
      },
      {
        "id": "destroyed-symlinks",
        "name": "detect destroyed symlinks",
        "description": "Detects symlinks that were replaced with regular files whose contents are the original symlink target path."
      },
      {
        "id": "detect-private-key",
        "name": "detect private key",
        "description": "Detects the presence of private keys."
      },
      {
        "id": "end-of-file-fixer",
        "name": "fix end of files",
        "description": "Ensures that a file is either empty, or ends with one newline."
      },
      {
        "id": "file-contents-sorter",
        "name": "file contents sorter",
        "description": "Sorts the lines in specified files (defaults to alphabetical)."
      },
      {
        "id": "fix-byte-order-marker",
        "name": "fix utf-8 byte order marker",
        "description": "Removes UTF-8 byte order marker."
      },
      {
        "id": "forbid-new-submodules",
        "name": "forbid new submodules",
        "description": "Prevents the addition of new Git submodules."
      },
      {
        "id": "mixed-line-ending",
        "name": "mixed line ending",
        "description": "Replaces or checks mixed line endings."
      },
      {
        "id": "no-commit-to-branch",
        "name": "don't commit to branch",
        "description": "Protects specific branches from direct commits."
      },
      {
        "id": "pretty-format-json",
        "name": "pretty format json",
        "description": "Checks that JSON files are pretty-formatted."
      },
      {
        "id": "require-filename-pattern",
        "name": "require filename patterns",
        "description": "Fails if any selected filename does not match a regular expression."
      },
      {
        "id": "require-pattern",
        "name": "require patterns",
        "description": "Fails if any file does not contain a matching regular expression."
      },
      {
        "id": "requirements-txt-fixer",
        "name": "fix requirements.txt",
        "description": "Sorts entries in requirements.txt."
      },
      {
        "id": "trailing-whitespace",
        "name": "trim trailing whitespace",
        "description": "Trims trailing whitespace."
      }
    ]

    ----- stderr -----
    "#);
}
