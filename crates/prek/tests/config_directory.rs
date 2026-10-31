mod common;

use indoc::indoc;

use crate::common::{TestEnv, cmd_snapshot};

/// All supported configs in `.config/` use their parent project's CWD and file scope.
/// Fresh and cached discovery agree, even when `.config/` is unignored or prek starts inside it.
#[test]
fn config_directory_project_roots() {
    let yaml = indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: show-cwd
                name: Show CWD
                language: system
                entry: python3 -c 'import os, sys; print(os.getcwd()); print(sorted(sys.argv[1:]))'
                files: '(^|/)file.txt$'
                verbose: true
    "};
    let toml = indoc! {r#"
        [[repos]]
        repo = "local"
        [[repos.hooks]]
        id = "show-cwd"
        name = "Show CWD"
        language = "system"
        entry = "python3 -c 'import os, sys; print(os.getcwd()); print(sorted(sys.argv[1:]))'"
        files = '(^|/)file.txt$'
        verbose = true
    "#};

    insta::allow_duplicates! {
        for (filename, config) in [
            ("prek.toml", toml),
            (".pre-commit-config.yaml", yaml),
            (".pre-commit-config.yml", yaml),
        ] {
            let context = TestEnv::new()
                .with_file(format!(".config/{filename}"), config)
                .with_file(format!("child/.config/{filename}"), config)
                .with_file("file.txt", "root\n")
                .with_file("child/file.txt", "child\n")
                .with_file("child/.config/deeper/other.txt", "config data\n")
                .with_file(".gitignore", "!.config/\n")
                .init_git();

            for refresh in [true, false] {
                let mut command = context.run();
                if refresh {
                    command.arg("--refresh");
                }
                cmd_snapshot!(context, command, @"
                success: true
                exit_code: 0
                ----- stdout -----
                ✓ child
                  Show CWD...............................................................Passed
                  - hook id: show-cwd
                  - duration: [TIME]

                    [TEMP_DIR]/child
                    ['file.txt']
                ✓ <workspace>
                  Show CWD...............................................................Passed
                  - hook id: show-cwd
                  - duration: [TIME]

                    [TEMP_DIR]/
                    ['child/file.txt', 'file.txt']

                ----- stderr -----
                ");
            }

            cmd_snapshot!(context, context.run().current_dir(context.work_dir().join("child/.config/deeper")), @"
            success: true
            exit_code: 0
            ----- stdout -----
            Show CWD.................................................................Passed
            - hook id: show-cwd
            - duration: [TIME]

              [TEMP_DIR]/child
              ['file.txt']

            ----- stderr -----
            ");
        }
    }
}

/// Root configs take precedence even when invoked from inside `.config/`.
/// Within `.config/`, TOML takes precedence over YAML; unused configs need not parse.
#[test]
fn config_directory_precedence() {
    let context = TestEnv::new()
        .with_config("repos: [{repo: meta, hooks: [{id: identity}]}]")
        .with_file(".config/prek.toml", "invalid TOML")
        .init_git();

    cmd_snapshot!(context, context.list().current_dir(context.work_dir().join(".config")), @"
    success: true
    exit_code: 0
    ----- stdout -----
    .:identity

    ----- stderr -----
    warning: Multiple configuration files found (`.pre-commit-config.yaml`, `.config/prek.toml`); using `[TEMP_DIR]/.pre-commit-config.yaml`
    ");

    context.git().rm(".pre-commit-config.yaml");
    context.write_file(
        ".config/prek.toml",
        indoc! {r#"
        [[repos]]
        repo = "meta"
        hooks = [{ id = "check-hooks-apply" }]
    "#},
    );
    context.write_file(".config/.pre-commit-config.yaml", "invalid YAML");

    cmd_snapshot!(context, context.list().arg("--refresh"), @"
    success: true
    exit_code: 0
    ----- stdout -----
    .:check-hooks-apply

    ----- stderr -----
    warning: Multiple configuration files found (`.config/prek.toml`, `.config/.pre-commit-config.yaml`); using `[TEMP_DIR]/.config/prek.toml`
    ");
}

/// Installation uses the root `.config/` config's hook types despite different child defaults.
/// The installed Git hook discovers that config and receives project-relative filenames.
#[test]
fn config_directory_installed_hooks() {
    let context = TestEnv::new()
        .with_file(
            ".config/prek.toml",
            indoc! {r#"
                default_install_hook_types = ["pre-commit", "pre-push"]
                [[repos]]
                repo = "meta"
                hooks = [{ id = "identity", files = '^file.txt$' }]
            "#},
        )
        .with_file(
            "child/prek.toml",
            "default_install_hook_types = ['commit-msg']\nrepos = []\n",
        )
        .with_file("file.txt", "content\n")
        .init_git();

    cmd_snapshot!(context, context.install(), @"
    success: true
    exit_code: 0
    ----- stdout -----
    Installed Git hook at `.git/hooks/pre-commit`
    Installed Git hook at `.git/hooks/pre-push`

    ----- stderr -----
    ");
    cmd_snapshot!(context, context.git().command().args(["hook", "run", "pre-commit"]), @"
    success: true
    exit_code: 0
    ----- stdout -----

    ----- stderr -----
    identity.................................................................Passed
    - hook id: identity
    - duration: [TIME]

      file.txt
    ");
}

/// Meta hooks recognize a child project's `.config/` config and match project-relative paths.
/// Both checks pass with matching files and fail after those files are removed.
#[test]
fn config_directory_meta_hooks() {
    let context = TestEnv::new()
        .with_config("repos: []")
        .with_file(
            "child/.config/prek.toml",
            indoc! {r#"
                exclude = '^excluded.txt$'
                [[repos]]
                repo = "meta"
                hooks = [{ id = "check-hooks-apply" }, { id = "check-useless-excludes" }]
                [[repos]]
                repo = "local"
                [[repos.hooks]]
                id = "check-file"
                name = "Check file"
                language = "system"
                entry = "echo"
                files = '^file.txt(.bak)?$'
                exclude = '^file.txt.bak$'
            "#},
        )
        .with_file("child/file.txt", "content\n")
        .with_file("child/file.txt.bak", "backup\n")
        .with_file("child/excluded.txt", "excluded\n")
        .init_git();

    cmd_snapshot!(context, context.run().args(["check-hooks-apply", "check-useless-excludes"]), @"
    success: true
    exit_code: 0
    ----- stdout -----
    ✓ child
      Check hooks apply......................................................Passed
      Check useless excludes.................................................Passed

    ----- stderr -----
    ");

    context
        .git()
        .rm("child/file.txt")
        .rm("child/file.txt.bak")
        .rm("child/excluded.txt");

    cmd_snapshot!(context, context.run().args(["check-hooks-apply", "check-useless-excludes"]), @"
    success: false
    exit_code: 1
    ----- stdout -----
    × child
      Check hooks apply......................................................Failed
      - hook id: check-hooks-apply
      - exit code: 1

        check-file does not apply to this repository
      Check useless excludes.................................................Failed
      - hook id: check-useless-excludes
      - exit code: 1

        The global exclude pattern `regex: ^excluded.txt$` does not match any files
        The exclude pattern `regex: ^file.txt.bak$` for `check-file` does not match any files

    ----- stderr -----
    ");
}

/// A Git repository named `.config/` contains both `.git/` and `prek.toml` directly.
/// Fresh and cached discovery must stay inside it; the invalid parent config catches traversal
/// beyond the Git root. Hook paths are relative to this root, so identity prints `prek.toml`.
#[test]
fn git_root_named_config_directory() {
    let context = TestEnv::new()
        .with_file(
            ".config/prek.toml",
            "[[repos]]\nrepo = 'meta'\nhooks = [{ id = 'identity' }]\n",
        )
        .with_config("invalid YAML");
    let git_root = context.work_dir().join(".config");
    context.git_at(&git_root).init().add(".");

    insta::allow_duplicates! {
        for refresh in [true, false] {
            let mut command = context.run();
            command.current_dir(&git_root);
            if refresh {
                command.arg("--refresh");
            }
            cmd_snapshot!(context, command, @"
            success: true
            exit_code: 0
            ----- stdout -----
            identity.................................................................Passed
            - hook id: identity
            - duration: [TIME]

              prek.toml

            ----- stderr -----
            ");
        }
    }
}
