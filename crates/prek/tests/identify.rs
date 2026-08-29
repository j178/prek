#[cfg(unix)]
use crate::common::{TestEnv, cmd_snapshot};
#[cfg(unix)]
mod common;

#[cfg(unix)] // "executable" tag is different on Windows
#[test]
fn identify_text_with_missing_paths() {
    let context = TestEnv::new().with_file("hello.py", "print('hi')\n");

    cmd_snapshot!(context,
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
    hello.py: file, non-executable, python, text

    ----- stderr -----
    error: missing.py: failed to query metadata of symlink `missing.py`: No such file or directory (os error 2)
    "
    );
}

#[cfg(unix)] // "executable" tag is different on Windows
#[test]
fn identify_json_with_missing_paths() {
    let context = TestEnv::new().with_file("hello.py", "print('hi')\n");

    cmd_snapshot!(context,
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
          "python",
          "text"
        ]
      }
    ]

    ----- stderr -----
    error: missing.py: failed to query metadata of symlink `missing.py`: No such file or directory (os error 2)
    "#);
}
