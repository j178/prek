mod common;

use crate::common::{TestEnv, cmd_snapshot};

/// Discovery reads ignored config symlinks without treating their hidden target directory as a project.
#[cfg(unix)]
#[test]
fn ignored_config_symlink() -> anyhow::Result<()> {
    let context = TestEnv::new()
        .with_config("repos: []")
        .with_file(
            ".shared/prek.toml",
            "[[repos]]\nrepo = 'meta'\nhooks = [{ id = 'identity' }]\n",
        )
        .with_file("file-link/file.txt", "")
        .with_file(".prekignore", "*.toml\n")
        .init_git();
    std::os::unix::fs::symlink(
        "../.shared/prek.toml",
        context.work_dir().join("file-link/prek.toml"),
    )?;

    cmd_snapshot!(context, context.list().arg("--refresh"), @"
    success: true
    exit_code: 0
    ----- stdout -----
    file-link:identity

    ----- stderr -----
    ");
    Ok(())
}

/// Directory enumeration preserves the filesystem's case and Unicode name matching.
/// Backup filenames do not become configs.
#[test]
fn filesystem_name_matching() {
    insta::allow_duplicates! {
        for filename in ["PREK.TOML", "pre\u{212a}.toml", "prek.toml.bak"] {
            let context = TestEnv::new()
                .with_config("repos: []")
                .with_file(
                    format!("child/{filename}"),
                    "[[repos]]\nrepo = 'meta'\nhooks = [{ id = 'identity' }]\n",
                )
                .init_git();

            if context.work_dir().join("child/prek.toml").is_file() {
                cmd_snapshot!(context, context.list().arg("--refresh"), @"
                success: true
                exit_code: 0
                ----- stdout -----
                child:identity

                ----- stderr -----
                ");
            } else {
                cmd_snapshot!(context, context.list().arg("--refresh"), @"
                success: true
                exit_code: 0
                ----- stdout -----

                ----- stderr -----
                ");
            }
        }
    }
}
