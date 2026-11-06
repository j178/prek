use crate::common::{TestEnv, cmd_snapshot};

#[test]
fn run_glob_patterns_with_multiple_hooks() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: echo-py
                name: echo-py
                entry: python3 -c "import sys; print('PY:' + ' '.join(sys.argv[2:]))" _
                language: system
                files:
                  glob: src/**/*.py
                verbose: true
              - id: echo-md
                name: echo-md
                entry: python3 -c "import sys; print('MD:' + ' '.join(sys.argv[2:]))" _
                language: system
                files:
                  glob: "**/*.md"
                verbose: true
    "#})
        .with_files([
            ("src/main.py", "print('hi')"),
            ("docs/readme.md", "# Docs"),
            ("notes.txt", "note"),
        ])
        .init_git();

    cmd_snapshot!(context, context.run().arg("--all-files"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    echo-py..................................................................Passed
    - hook id: echo-py
    - duration: [TIME]

      PY:src/main.py
    echo-md..................................................................Passed
    - hook id: echo-md
    - duration: [TIME]

      MD:docs/readme.md

    ----- stderr -----
    ");
}

/// Test global `files`, `exclude`, and hook level `files`, `exclude`.
#[test]
fn files_and_exclude() {
    let context = TestEnv::new()
        .with_file("file.txt", "Hello, world!  \n")
        .with_file("valid.json", "{}\n  ")
        .with_file("invalid.json", "{}")
        .with_file("main.py", r#"print "abc"  "#)
        .init_git();

    // Global files and exclude.
    context.write_config(indoc::indoc! {r"
        files: file.txt
        repos:
          - repo: local
            hooks:
              - id: trailing-whitespace
                name: trailing whitespace
                language: system
                entry: python3 -c 'import sys; print(sys.argv[1:]); exit(1)'
                types: [text]
              - id: end-of-file-fixer
                name: fix end of files
                language: system
                entry: python3 -c 'import sys; print(sys.argv[1:]); exit(1)'
                types: [text]
              - id: check-json
                name: check json
                language: system
                entry: python3 -c 'import sys; print(sys.argv[1:]); exit(1)'
                types: [json]
    "});
    context.git().add(".");

    cmd_snapshot!(context, context.run(), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    trailing whitespace......................................................Failed
    - hook id: trailing-whitespace
    - exit code: 1

      ['file.txt']
    fix end of files.........................................................Failed
    - hook id: end-of-file-fixer
    - exit code: 1

      ['file.txt']
    check json...........................................(no files to check)Skipped

    ----- stderr -----
    ");

    // Override hook level files and exclude.
    context.write_config(indoc::indoc! {r"
        files: file.txt
        repos:
          - repo: local
            hooks:
              - id: trailing-whitespace
                name: trailing whitespace
                language: system
                entry: python3 -c 'import sys; print(sys.argv[1:]); exit(1)'
                files: valid.json
              - id: end-of-file-fixer
                name: fix end of files
                language: system
                entry: python3 -c 'import sys; print(sys.argv[1:]); exit(1)'
                exclude: (valid.json|main.py)
              - id: check-json
                name: check json
                language: system
                entry: python3 -c 'import sys; print(sys.argv[1:]); exit(1)'
    "});
    context.git().add(".");

    cmd_snapshot!(context, context.run(), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    trailing whitespace..................................(no files to check)Skipped
    fix end of files.........................................................Failed
    - hook id: end-of-file-fixer
    - exit code: 1

      ['file.txt']
    check json...............................................................Failed
    - hook id: check-json
    - exit code: 1

      ['file.txt']

    ----- stderr -----
    ");
}

/// Test selecting files by type, `types`, `types_or`, and `exclude_types`.
#[test]
fn file_types() {
    let context = TestEnv::new()
        .with_file("file.txt", "Hello, world!  ")
        .with_file("json.json", "{}\n  ")
        .with_file("main.py", r#"print "abc"  "#)
        .init_git();

    context.write_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: trailing-whitespace
                name: trailing-whitespace
                language: system
                entry: python3 -c 'import sys; print(sys.argv[1:]); exit(1)'
                types: ["json"]
          - repo: local
            hooks:
              - id: trailing-whitespace
                name: trailing-whitespace
                language: system
                entry: python3 -c 'import sys; print(sys.argv[1:]); exit(1)'
                types_or: ["json", "python"]
          - repo: local
            hooks:
              - id: trailing-whitespace
                name: trailing-whitespace
                language: system
                entry: python3 -c 'import sys; print(sys.argv[1:]); exit(1)'
                exclude_types: ["json"]
          - repo: local
            hooks:
              - id: trailing-whitespace
                name: trailing-whitespace
                language: system
                entry: python3 -c 'import sys; print(sys.argv[1:]); exit(1)'
                types: ["json" ]
                exclude_types: ["json"]
    "#});
    context.git().add(".");

    cmd_snapshot!(context, context.run(), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    trailing-whitespace......................................................Failed
    - hook id: trailing-whitespace
    - exit code: 1

      ['json.json']
    trailing-whitespace......................................................Failed
    - hook id: trailing-whitespace
    - exit code: 1

      ['json.json', 'main.py']
    trailing-whitespace......................................................Failed
    - hook id: trailing-whitespace
    - exit code: 1

      ['.pre-commit-config.yaml', 'file.txt', 'main.py']
    trailing-whitespace..................................(no files to check)Skipped

    ----- stderr -----
    "#);
}

/// Run from a subdirectory. File arguments should be fixed to be relative to the root.
#[test]
fn subdirectory() {
    let context = TestEnv::new()
        .with_file("foo/bar/baz/file.txt", "Hello, world!\n")
        .with_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: trailing-whitespace
                name: trailing-whitespace
                language: system
                entry: python3 -c 'import sys; print(sys.argv[1]); exit(1)'
                always_run: true
    "})
        .init_git();
    let child = context.child("foo/bar/baz");

    context.git().add(".");

    cmd_snapshot!(context, context.run().current_dir(&child).arg("--files").arg("file.txt"), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    trailing-whitespace......................................................Failed
    - hook id: trailing-whitespace
    - exit code: 1

      foo/bar/baz/file.txt

    ----- stderr -----
    ");

    cmd_snapshot!(context, context.run().arg("--cd").arg(&*child).arg("--files").arg("file.txt"), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    trailing-whitespace......................................................Failed
    - hook id: trailing-whitespace
    - exit code: 1

      foo/bar/baz/file.txt

    ----- stderr -----
    ");
}

/// Test hooks that specifies `types: [directory]`.
#[test]
fn types_directory() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: directory
                name: directory
                language: system
                entry: echo
                types: [directory]
        "})
        .with_file("dir/file.txt", "Hello, world!")
        .init_git();

    cmd_snapshot!(context, context.run(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    directory............................................(no files to check)Skipped

    ----- stderr -----
    "#);

    cmd_snapshot!(context, context.run().arg("--files").arg("dir"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    directory................................................................Passed

    ----- stderr -----
    "#);

    cmd_snapshot!(context, context.run().arg("--all-files"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    directory............................................(no files to check)Skipped

    ----- stderr -----
    "#);

    cmd_snapshot!(context, context.run().arg("--files").arg("non-exist-files"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    directory............................................(no files to check)Skipped

    ----- stderr -----
    warning: This file does not exist and will be ignored: `non-exist-files`
    ");
}

/// Test `prek run --files` with multiple files.
#[test]
fn run_multiple_files() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: multiple-files
                name: multiple-files
                language: system
                entry: echo
                verbose: true
                types: [text]
    "})
        .with_file("file1.txt", "Hello, world!")
        .with_file("file2.txt", "Hello, world!")
        .init_git();

    // `--files` with multiple files
    cmd_snapshot!(context, context.run().arg("--files").arg("file1.txt").arg("file2.txt"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    multiple-files...........................................................Passed
    - hook id: multiple-files
    - duration: [TIME]

      file1.txt file2.txt

    ----- stderr -----
    "#);
}

/// A list file and its entries use `--cd`, and all explicit selectors share deduplication.
#[test]
fn run_files0_from() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: filenames
                name: filenames
                language: system
                entry: python3 -c 'import json, sys; print(json.dumps(sorted(sys.argv[1:])))'
                verbose: true
    "})
        .with_files([
            ("root.txt", "root\n"),
            ("src/tracked.txt", "tracked\n"),
            ("docs/readme.txt", "docs\n"),
        ])
        .init_git()
        .with_file("src/untracked.txt", "untracked\n")
        .with_file("lists/files0", b"untracked.txt\0missing.txt\0../root.txt");

    cmd_snapshot!(context, context.run()
        .args(["--cd", "src", "--files0-from", "../lists/files0"])
        .args(["--files", "untracked.txt", "--glob", "*.txt", "--directory", "../docs"]), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    filenames................................................................Passed
    - hook id: filenames
    - duration: [TIME]

      ["docs/readme.txt", "root.txt", "src/tracked.txt", "src/untracked.txt"]

    ----- stderr -----
    warning: This file does not exist and will be ignored: `missing.txt`
    "#);
}

/// Piped paths are literal, and EOF can terminate the last path.
#[test]
fn run_files0_from_stdin() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: filenames
                name: filenames
                language: system
                entry: python3 -c 'import json, sys; print(json.dumps(sorted(sys.argv[1:])))'
                verbose: true
    "})
        .with_files([("hello world.txt", "hello\n"), ("quote's.txt", "quote\n")])
        .init_git();

    cmd_snapshot!(context, context.run()
        .args(["--files0-from", "-"])
        .pass_stdin("hello world.txt\0quote's.txt"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    filenames................................................................Passed
    - hook id: filenames
    - duration: [TIME]

      ["hello world.txt", "quote's.txt"]

    ----- stderr -----
    "#);
}

/// An empty list must not select staged files or stash edits; `always_run` still applies.
#[test]
fn run_files0_from_empty() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: filenames
                name: filenames
                language: system
                entry: echo
                files: \.txt$
              - id: always
                name: always
                language: system
                entry: python3 -c 'import sys; print(sys.argv[1:])'
                always_run: true
                verbose: true
    "})
        .with_file("staged.txt", "staged\n")
        .init_git()
        .with_file("staged.txt", "unstaged\n")
        .with_file("files0", b"");

    cmd_snapshot!(context, context.run().args(["--files0-from", "files0"]), @"
    success: true
    exit_code: 0
    ----- stdout -----
    filenames............................................(no files to check)Skipped
    always...................................................................Passed
    - hook id: always
    - duration: [TIME]

      []

    ----- stderr -----
    ");
}

/// Unreadable or malformed lists fail before any hook runs, even after valid entries.
#[test]
fn run_files0_from_errors() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: must-not-run
                name: must-not-run
                language: system
                entry: echo
                always_run: true
    "})
        .with_file("exists.txt", "exists\n")
        .init_git();

    cmd_snapshot!(context, context.run().args(["--files0-from", "missing"]), @"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Failed to collect files
      caused by: failed to read from file `missing`: No such file or directory (os error 2)
    ");

    context.write_file("files0", b"\0");
    cmd_snapshot!(context, context.run().args(["--files0-from", "files0"]), @"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Failed to collect files
      caused by: File list `files0` contains an empty path at entry 1
    ");

    context.write_file("files0", b"exists.txt\0\0");
    cmd_snapshot!(context, context.run().args(["--files0-from", "files0"]), @"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Failed to collect files
      caused by: File list `files0` contains an empty path at entry 2
    ");
}

/// Unix paths retain whitespace and shell metacharacters through hook execution.
#[cfg(unix)]
#[test]
fn run_files0_from_preserves_unix_paths() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt as _;

    // Hex output prevents the snapshot path filters from rewriting literal backslashes.
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: filenames
                name: filenames
                language: system
                entry: python3 -c 'import os, sys; print("\n".join(path.hex() for path in sorted(os.fsencode(arg) for arg in sys.argv[1:])))'
                verbose: true
    "#})
        .init_git();

    let names: &[&[u8]] = &[
        b" leading.txt",
        b"trailing.txt ",
        b"line\nbreak.txt",
        b"tab\tname.txt",
        b"single'quote.txt",
        b"double\"quote.txt",
        b"back\\slash.txt",
        b"$(literal)*.txt",
        b"-leading-dash.txt",
    ];
    let mut files0 = Vec::new();
    for name in names {
        context.write_file(OsStr::from_bytes(name), "content\n");
        files0.extend_from_slice(name);
        files0.push(b'\0');
    }
    context.write_file("files0", files0);

    cmd_snapshot!(context, context.run().args(["--files0-from", "files0"]), @"
    success: true
    exit_code: 0
    ----- stdout -----
    filenames................................................................Passed
    - hook id: filenames
    - duration: [TIME]

      206c656164696e672e747874
      24286c69746572616c292a2e747874
      2d6c656164696e672d646173682e747874
      6261636b5c736c6173682e747874
      646f75626c652271756f74652e747874
      6c696e650a627265616b2e747874
      73696e676c652771756f74652e747874
      746162096e616d652e747874
      747261696c696e672e74787420

    ----- stderr -----
    ");
}

/// Test `prek run --glob` and its interaction with other explicit file selectors.
#[test]
fn run_glob() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: glob
                name: glob
                language: system
                entry: echo
                verbose: true
                types: [text]
    "})
        .with_files([
            ("root.rs", "fn main() {}"),
            ("src/lib.rs", "pub fn lib() {}"),
            ("src/lib.py", "print('hello')"),
            ("src/nested/mod.rs", "pub mod nested;"),
            ("docs/readme.md", "# Readme"),
        ])
        .init_git();

    context.write_file("src/untracked.rs", "pub fn untracked() {}");

    cmd_snapshot!(context, context.run().arg("--glob").arg("src/**/*.rs"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    glob.....................................................................Passed
    - hook id: glob
    - duration: [TIME]

      src/lib.rs src/nested/mod.rs

    ----- stderr -----
    "#);

    cmd_snapshot!(context, context.run().arg("--cd").arg("src").arg("--files").arg("../root.rs").arg("--directory").arg("../docs").arg("--glob").arg("**/*.rs"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    glob.....................................................................Passed
    - hook id: glob
    - duration: [TIME]

      docs/readme.md root.rs src/nested/mod.rs src/lib.rs

    ----- stderr -----
    "#);

    cmd_snapshot!(context, context.run().arg("--cd").arg("src").arg("--glob").arg("../*.rs"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    glob.................................................(no files to check)Skipped

    ----- stderr -----
    ");

    cmd_snapshot!(context, context.run().arg("--glob").arg("missing/**/*.rs"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    glob.................................................(no files to check)Skipped

    ----- stderr -----
    ");
}

/// Test `prek run --files` with no files.
#[test]
fn run_no_files() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: no-files
                name: no-files
                language: system
                entry: echo
                verbose: true
    "})
        .init_git();

    // `--files` with no files
    cmd_snapshot!(context, context.run().arg("--files"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    no-files.................................................................Passed
    - hook id: no-files
    - duration: [TIME]

      .pre-commit-config.yaml

    ----- stderr -----
    ");
}

/// Test `prek run --directory` flags.
#[test]
fn run_directory() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: directory
                name: directory
                language: system
                entry: echo
                verbose: true
    "})
        .with_file("dir1/file.txt", "Hello, world!")
        .with_file("dir2/file.txt", "Hello, world!")
        .init_git();
    let cwd = context.work_dir();

    context.git().add(".");

    // one `--directory`
    cmd_snapshot!(context, context.run().arg("--directory").arg("dir1"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    directory................................................................Passed
    - hook id: directory
    - duration: [TIME]

      dir1/file.txt

    ----- stderr -----
    ");

    // repeated `--directory`
    cmd_snapshot!(context, context.run().arg("--directory").arg("dir1").arg("--directory").arg("dir1"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    directory................................................................Passed
    - hook id: directory
    - duration: [TIME]

      dir1/file.txt

    ----- stderr -----
    ");

    // multiple `--directory`
    cmd_snapshot!(context, context.run().arg("--directory").arg("dir1").arg("--directory").arg("dir2"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    directory................................................................Passed
    - hook id: directory
    - duration: [TIME]

      dir1/file.txt dir2/file.txt

    ----- stderr -----
    "#);

    // non-existing directory
    cmd_snapshot!(context, context.run().arg("--directory").arg("non-existing-dir"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    directory............................................(no files to check)Skipped

    ----- stderr -----
    ");

    // `--directory` with `--files`
    cmd_snapshot!(context, context.run().arg("--directory").arg("dir1").arg("--files").arg("dir1/file.txt"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    directory................................................................Passed
    - hook id: directory
    - duration: [TIME]

      dir1/file.txt

    ----- stderr -----
    ");
    cmd_snapshot!(context, context.run().arg("--directory").arg("dir1").arg("--files").arg("dir2/file.txt"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    directory................................................................Passed
    - hook id: directory
    - duration: [TIME]

      dir1/file.txt dir2/file.txt

    ----- stderr -----
    "#);

    // run `--directory` inside a subdirectory
    cmd_snapshot!(context, context.run().current_dir(cwd.join("dir1")).arg("--directory").arg("."), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    directory................................................................Passed
    - hook id: directory
    - duration: [TIME]

      dir1/file.txt

    ----- stderr -----
    ");

    cmd_snapshot!(context, context.run().arg("--cd").arg("dir1").arg("--directory").arg("."), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    directory................................................................Passed
    - hook id: directory
    - duration: [TIME]

      dir1/file.txt

    ----- stderr -----
    ");
}
