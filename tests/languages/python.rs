use crate::common::{TestContext, cmd_snapshot};

/// Test `language_version` parsing.
#[test]
fn language_version() {
    let context = TestContext::new();
    context.init_project();
    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: python3
                name: python3
                language: python
                entry: python -c 'print("Hello, World!")'
                language_version: python3
                always_run: true
              - id: python3.12
                name: python3.12
                language: python
                entry: python -c 'import sys; print(sys.version_info[:3])'
                language_version: python3.12
                always_run: true
              - id: python3.12
                name: python3.12
                language: python
                entry: python -c 'import sys; print(sys.version_info[:3])'
                language_version: '3.12'
                always_run: true
              - id: python3.12
                name: python3.12
                language: python
                entry: python -c 'import sys; print(sys.version_info[:3])'
                language_version: managed; python3.12
                always_run: true
              - id: greater-than-python3.13
                name: greater-than-python3.13
                language: python
                entry: python -c 'import sys; print(sys.version_info[:3])'
                language_version: '>=3.13'
                always_run: true
        # TODO: Fix python auto download support, then enable below tests
        #- id: python3.12
        #  name: python3.12
        #  language: python
        #  entry: python -c 'import sys; print(sys.version_info[:3])'
        #  language_version: only-managed; 3.12
        #  always_run: true
        #- id: greater-than-python3.13
        #  name: greater-than-python3.13
        #  language: python
        #  entry: python -c 'import sys; print(sys.version_info[:3])'
        #  language_version: only-managed; >=3.13
        #  always_run: true
    "#});
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run().arg("-v"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    python3..................................................................Passed
    - hook id: python3
    - duration: [TIME]
      Hello, World!
    python3.12...............................................................Passed
    - hook id: python3.12
    - duration: [TIME]
      (3, 12, 11)
    python3.12...............................................................Passed
    - hook id: python3.12
    - duration: [TIME]
      (3, 12, 11)
    python3.12...............................................................Passed
    - hook id: python3.12
    - duration: [TIME]
      (3, 12, 11)
    greater-than-python3.13..................................................Passed
    - hook id: greater-than-python3.13
    - duration: [TIME]
      (3, 13, 5)

    ----- stderr -----
    "#);
}
