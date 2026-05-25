use crate::common::{TestContext, cmd_snapshot};

mod common;

#[test]
fn global_config_missing_file_is_optional() {
    let context = TestContext::new();
    context.init_project();
    context.write_pre_commit_config("repos: []");

    cmd_snapshot!(context.filters(), context.auto_update(), @"
    success: true
    exit_code: 0
    ----- stdout -----

    ----- stderr -----
    ");
}

#[test]
fn global_config_ignores_unknown_options() {
    let context = TestContext::new();
    context.init_project();
    context.write_pre_commit_config("repos: []");
    context.write_user_config(indoc::indoc! {r#"
        future_option = true

        [auto_update]
        cooldown_days = 3
        future_option = "ignored"
    "#});

    cmd_snapshot!(context.filters(), context.auto_update(), @"
    success: true
    exit_code: 0
    ----- stdout -----

    ----- stderr -----
    ");
}

#[test]
fn global_config_invalid_file_reports_parse_error() {
    let context = TestContext::new();
    context.init_project();
    context.write_pre_commit_config("repos: []");
    context.write_user_config(indoc::indoc! {r#"
        [auto_update]
        cooldown_days = "soon"
    "#});

    cmd_snapshot!(context.filters(), context.auto_update(), @r#"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Failed to parse global config `[HOME]/config/prek/prek.toml`
      caused by: TOML parse error at line 2, column 17
      |
    2 | cooldown_days = "soon"
      |                 ^^^^^^
    invalid type: string "soon", expected u8
    "#);
}
