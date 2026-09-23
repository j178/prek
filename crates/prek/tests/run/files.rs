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
