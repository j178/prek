use crate::common::{TestContext, cmd_snapshot};

mod common;

#[test]
fn list_builtins_basic() {
    let context = TestContext::new();

    cmd_snapshot!(context.filters(), context.list_builtins(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    check-added-large-files
    check-case-conflict
    check-executables-have-shebangs
    check-json
    check-json5
    check-merge-conflict
    check-symlinks
    check-toml
    check-xml
    check-yaml
    detect-private-key
    end-of-file-fixer
    fix-byte-order-marker
    mixed-line-ending
    no-commit-to-branch
    trailing-whitespace

    ----- stderr -----
    ");
}

#[test]
fn list_builtins_verbose() {
    let context = TestContext::new();

    cmd_snapshot!(context.filters(), context.list_builtins().arg("--verbose"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    check-added-large-files
      prevents giant files from being committed.

    check-case-conflict
      checks for files that would conflict in case-insensitive filesystems

    check-executables-have-shebangs
      ensures that (non-binary) executables have a shebang.

    check-json
      checks json files for parseable syntax.

    check-json5
      checks json5 files for parseable syntax.

    check-merge-conflict
      checks for files that contain merge conflict strings.

    check-symlinks
      checks for symlinks which do not point to anything.

    check-toml
      checks toml files for parseable syntax.

    check-xml
      checks xml files for parseable syntax.

    check-yaml
      checks yaml files for parseable syntax.

    detect-private-key
      detects the presence of private keys.

    end-of-file-fixer
      ensures that a file is either empty, or ends with one newline.

    fix-byte-order-marker
      removes utf-8 byte order marker.

    mixed-line-ending
      replaces or checks mixed line ending.

    no-commit-to-branch

    trailing-whitespace
      trims trailing whitespace.


    ----- stderr -----
    ");
}

#[test]
fn list_builtins_json() {
    let context = TestContext::new();

    cmd_snapshot!(context.filters(), context.list_builtins().arg("--output-format=json"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    [
      {
        "id": "check-added-large-files",
        "name": "check for added large files",
        "description": "prevents giant files from being committed."
      },
      {
        "id": "check-case-conflict",
        "name": "check for case conflicts",
        "description": "checks for files that would conflict in case-insensitive filesystems"
      },
      {
        "id": "check-executables-have-shebangs",
        "name": "check that executables have shebangs",
        "description": "ensures that (non-binary) executables have a shebang."
      },
      {
        "id": "check-json",
        "name": "check json",
        "description": "checks json files for parseable syntax."
      },
      {
        "id": "check-json5",
        "name": "check json5",
        "description": "checks json5 files for parseable syntax."
      },
      {
        "id": "check-merge-conflict",
        "name": "check for merge conflicts",
        "description": "checks for files that contain merge conflict strings."
      },
      {
        "id": "check-symlinks",
        "name": "check for broken symlinks",
        "description": "checks for symlinks which do not point to anything."
      },
      {
        "id": "check-toml",
        "name": "check toml",
        "description": "checks toml files for parseable syntax."
      },
      {
        "id": "check-xml",
        "name": "check xml",
        "description": "checks xml files for parseable syntax."
      },
      {
        "id": "check-yaml",
        "name": "check yaml",
        "description": "checks yaml files for parseable syntax."
      },
      {
        "id": "detect-private-key",
        "name": "detect private key",
        "description": "detects the presence of private keys."
      },
      {
        "id": "end-of-file-fixer",
        "name": "fix end of files",
        "description": "ensures that a file is either empty, or ends with one newline."
      },
      {
        "id": "fix-byte-order-marker",
        "name": "fix utf-8 byte order marker",
        "description": "removes utf-8 byte order marker."
      },
      {
        "id": "mixed-line-ending",
        "name": "mixed line ending",
        "description": "replaces or checks mixed line ending."
      },
      {
        "id": "no-commit-to-branch",
        "name": "don't commit to branch",
        "description": null
      },
      {
        "id": "trailing-whitespace",
        "name": "trim trailing whitespace",
        "description": "trims trailing whitespace."
      }
    ]

    ----- stderr -----
    "#);
}
