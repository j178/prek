#[cfg(unix)]
use crate::common::{TestContext, cmd_snapshot};
#[cfg(unix)]
use assert_fs::fixture::{FileWriteStr, PathChild};

mod common;

#[cfg(unix)] // "executable" tag is different on Windows
#[test]
fn identify_text_with_missing_paths() -> anyhow::Result<()> {
    let context = TestContext::new();
    context
        .work_dir()
        .child("hello.py")
        .write_str("print('hi')\n")?;

    cmd_snapshot!(
        context.filters(),
        context
            .command()
            .arg("util")
            .arg("identify")
            .arg(".")
            .arg("hello.py")
            .arg("missing.py"),
        @"
    success: false
    exit_code: 1
    ----- stdout -----
    .: directory
    hello.py: file, non-executable, text, python

    ----- stderr -----
    error: missing.py: No such file or directory (os error 2)
    "
    );

    Ok(())
}

#[cfg(unix)] // "executable" tag is different on Windows
#[test]
fn identify_json_with_missing_paths() -> anyhow::Result<()> {
    let context = TestContext::new();
    context
        .work_dir()
        .child("hello.py")
        .write_str("print('hi')\n")?;

    cmd_snapshot!(
        context.filters(),
        context
            .command()
            .arg("util")
            .arg("identify")
            .arg("--output-format")
            .arg("json")
            .arg(".")
            .arg("hello.py")
            .arg("missing.py"),
        @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    [
      {
        "path": ".",
        "tags": [
          "directory"
        ]
      },
      {
        "path": "hello.py",
        "tags": [
          "file",
          "non-executable",
          "text",
          "python"
        ]
      }
    ]

    ----- stderr -----
    error: missing.py: No such file or directory (os error 2)
    "#);

    Ok(())
}
