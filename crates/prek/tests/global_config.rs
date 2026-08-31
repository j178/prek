use crate::common::{TestEnv, cmd_snapshot};

mod common;

#[test]
fn global_config_missing_file_is_optional() {
    let context = TestEnv::new().with_config("repos: []").init_git();

    cmd_snapshot!(context, context.update(), @"
    success: true
    exit_code: 0
    ----- stdout -----

    ----- stderr -----
    ");
}

#[test]
fn global_config_warns_about_unknown_options() {
    let context = TestEnv::new().with_config("repos: []").init_git();
    context.write_user_config(indoc::indoc! {r#"
        future_date = 1979-05-27T07:32:00Z
        future_option = { nested = [true, { value = 1 }] }

        [update]
        cooldown_days = 3
        future_option = ["ignored", { nested = true }]
    "#});

    cmd_snapshot!(context, context.update(), @"
    success: true
    exit_code: 0
    ----- stdout -----

    ----- stderr -----
    warning: Ignored unexpected keys in `[HOME]/config/prek/prek.toml`: `future_date`, `future_option`, `update.future_option`
    ");
}

#[test]
fn update_command_accepts_upstream_alias() {
    let context = TestEnv::new().with_config("repos: []").init_git();

    cmd_snapshot!(context, context.command().arg("autoupdate"), @"
    success: true
    exit_code: 0
    ----- stdout -----

    ----- stderr -----
    ");
}

#[test]
fn global_config_invalid_file_reports_parse_error() {
    let context = TestEnv::new().with_config("repos: []").init_git();
    context.write_user_config(indoc::indoc! {r#"
        [update]
        cooldown_days = "soon"
    "#});

    cmd_snapshot!(context, context.update(), @r#"
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
