#[cfg(unix)]
use anyhow::Result;
use prek_consts::PRE_COMMIT_CONFIG_YAML;

use crate::common::{TestEnv, cmd_snapshot};

#[test]
fn deleted_files_follow_path_and_type_filters() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        files: '\.(rs|md)$'
        exclude: ^vendor/
        repos:
          - repo: local
            hooks:
              - id: existing
                name: existing
                language: system
                entry: python3 -c 'import sys; print(sys.argv[1:])'
                types: [rust]
                verbose: true
              - id: deleted
                name: deleted
                language: system
                entry: python3 -c 'import sys; print(sys.argv[1:])'
                include_deleted: true
                types_or: [rust, markdown]
                exclude_types: [markdown]
                exclude: ignored
                verbose: true
              - id: project
                name: project
                language: system
                entry: python3 -c 'import sys; print(sys.argv[1:])'
                include_deleted: true
                types: [rust]
                pass_filenames: false
                verbose: true
        "})
        .with_files([
            ("module.rs", "pub fn removed() {}\n"),
            ("ignored.rs", "// ignored\n"),
            ("vendor/other.rs", "// excluded globally\n"),
            ("README.md", "# Readme\n"),
            ("script.py", "print('outside the global filter')\n"),
        ])
        .init_git();
    context.git().commit("Initial files");
    for file in [
        "module.rs",
        "ignored.rs",
        "vendor/other.rs",
        "README.md",
        "script.py",
    ] {
        context.git().rm(file);
    }

    cmd_snapshot!(context, context.run(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    existing.............................................(no files to check)Skipped
    deleted..................................................................Passed
    - hook id: deleted
    - duration: [TIME]

      ['module.rs']
    project..................................................................Passed
    - hook id: project
    - duration: [TIME]

      []

    ----- stderr -----
    "#);

    context.git().commit("Delete files");
    cmd_snapshot!(context, context.run().args(["--from-ref", "HEAD^", "--to-ref", "HEAD"]), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    existing.............................................(no files to check)Skipped
    deleted..................................................................Passed
    - hook id: deleted
    - duration: [TIME]

      ['module.rs']
    project..................................................................Passed
    - hook id: project
    - duration: [TIME]

      []

    ----- stderr -----
    "#);

    // Tree references use the two-dot fallback instead of a merge-base diff.
    cmd_snapshot!(context, context.run().args(["--from-ref", "HEAD~1^{tree}", "--to-ref", "HEAD^{tree}"]), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    existing.............................................(no files to check)Skipped
    deleted..................................................................Passed
    - hook id: deleted
    - duration: [TIME]

      ['module.rs']
    project..................................................................Passed
    - hook id: project
    - duration: [TIME]

      []

    ----- stderr -----
    "#);
}

#[test]
fn deleted_and_existing_files_share_arguments() {
    let context = TestEnv::new()
        .with_file("prek.toml", indoc::indoc! {r#"
        [[repos]]
        repo = "local"
        hooks = [
          { id = "default", name = "default", language = "system", entry = "python3 -c 'import sys; print(sys.argv[1:])'", types = ["rust"], verbose = true },
          { id = "included", name = "included", language = "system", entry = "python3 -c 'import sys; print(sys.argv[1:])'", types = ["rust"], include_deleted = true, verbose = true },
        ]
        "#})
        .with_files([("removed.rs", "// removed\n"), ("kept.rs", "// kept\n")])
        .init_git();
    context.git().commit("Initial files");
    context.git().rm("removed.rs");
    context.write_file("kept.rs", "// changed\n");
    context.git().add("kept.rs");

    cmd_snapshot!(context, context.run(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    default..................................................................Passed
    - hook id: default
    - duration: [TIME]

      ['kept.rs']
    included.................................................................Passed
    - hook id: included
    - duration: [TIME]

      ['kept.rs', 'removed.rs']

    ----- stderr -----
    "#);

    // Full and explicit file selections do not add unrelated staged deletions.
    cmd_snapshot!(context, context.run().arg("--all-files"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    default..................................................................Passed
    - hook id: default
    - duration: [TIME]

      ['kept.rs']
    included.................................................................Passed
    - hook id: included
    - duration: [TIME]

      ['kept.rs']

    ----- stderr -----
    "#);

    cmd_snapshot!(context, context.run().args(["--files", "kept.rs", "missing.rs"]), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    default..................................................................Passed
    - hook id: default
    - duration: [TIME]

      ['kept.rs']
    included.................................................................Passed
    - hook id: included
    - duration: [TIME]

      ['kept.rs']

    ----- stderr -----
    warning: This file does not exist and will be ignored: `missing.rs`
    "#);
}

#[test]
fn deleted_rename_source_triggers_hook() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: existing
                name: existing
                language: system
                entry: python3 -c 'import sys; print(sys.argv[1:])'
                types: [text]
                verbose: true
              - id: python
                name: python
                language: system
                entry: python3 -c 'import sys; print(sys.argv[1:])'
                include_deleted: true
                types: [python]
                verbose: true
        "})
        .with_file("script.py", "print('hello')\n")
        .init_git();
    context.git().commit("Initial files");
    context.git().run(["mv", "script.py", "script.txt"]);

    cmd_snapshot!(context, context.run(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    existing.................................................................Passed
    - hook id: existing
    - duration: [TIME]

      ['script.txt']
    python...................................................................Passed
    - hook id: python
    - duration: [TIME]

      ['script.py']

    ----- stderr -----
    "#);
}

#[test]
fn deleted_files_respect_workspace_boundaries() {
    let config = indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: rust
                name: rust
                language: system
                entry: python3 -c 'import sys; print(sys.argv[1:])'
                include_deleted: true
                types: [rust]
                verbose: true
    "};
    let context = TestEnv::new()
        .with_config(config)
        .with_file(
            format!("child/{PRE_COMMIT_CONFIG_YAML}"),
            format!("orphan: true\n{config}"),
        )
        .with_files([("root.rs", "// root\n"), ("child/nested.rs", "// child\n")])
        .init_git();
    context.git().commit("Initial files");
    context.git().rm("root.rs").rm("child/nested.rs");

    cmd_snapshot!(context, context.run(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    ✓ child
      rust...................................................................Passed
      - hook id: rust
      - duration: [TIME]

        ['nested.rs']
    ✓ <workspace>
      rust...................................................................Passed
      - hook id: rust
      - duration: [TIME]

        ['root.rs']

    ----- stderr -----
    "#);

    cmd_snapshot!(context, context.run().current_dir(context.work_dir().join("child")), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    rust.....................................................................Passed
    - hook id: rust
    - duration: [TIME]

      ['nested.rs']

    ----- stderr -----
    "#);

    context.git().commit("Delete files");
    cmd_snapshot!(context, context.run()
        .current_dir(context.work_dir().join("child"))
        .args(["--from-ref", "HEAD^", "--to-ref", "HEAD"]), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    rust.....................................................................Passed
    - hook id: rust
    - duration: [TIME]

      ['nested.rs']

    ----- stderr -----
    "#);
}

#[cfg(unix)]
#[test]
fn deleted_tags_use_git_mode_without_reading_content() -> Result<()> {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: rust
                name: rust
                language: system
                entry: python3 -c 'import sys; print(sys.argv[1:])'
                include_deleted: true
                types: [rust]
                verbose: true
              - id: symlink
                name: symlink
                language: system
                entry: python3 -c 'import sys; print(sys.argv[1:])'
                include_deleted: true
                types: [symlink]
                verbose: true
              - id: executable
                name: executable
                language: system
                entry: python3 -c 'import sys; print(sys.argv[1:])'
                include_deleted: true
                types: [file, executable]
                verbose: true
              - id: python
                name: python
                language: system
                entry: python3 -c 'print("ran")'
                include_deleted: true
                types: [python]
        "#})
        .with_file("module.rs", "// module\n")
        .with_executable_file("tool", "#!/usr/bin/env python3\nprint('hello')\n");
    std::os::unix::fs::symlink("module.rs", context.work_dir().join("link.rs"))?;
    let context = context.init_git();
    context.git().commit("Initial files");
    context.git().run(["rm", "module.rs", "link.rs", "tool"]);

    cmd_snapshot!(context, context.run(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    rust.....................................................................Passed
    - hook id: rust
    - duration: [TIME]

      ['module.rs']
    symlink..................................................................Passed
    - hook id: symlink
    - duration: [TIME]

      ['link.rs']
    executable...............................................................Passed
    - hook id: executable
    - duration: [TIME]

      ['tool']
    python...............................................(no files to check)Skipped

    ----- stderr -----
    "#);
    Ok(())
}
