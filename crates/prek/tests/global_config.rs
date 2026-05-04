use crate::common::{TestContext, cmd_snapshot};

mod common;

#[test]
fn global_config_missing_file_uses_defaults() {
    let context = TestContext::new();

    cmd_snapshot!(context.filters(), context.auto_update().arg("--show-settings"), @"
    success: true
    exit_code: 0
    ----- stdout -----
    GlobalArgs {
        config: None,
        cd: None,
        color: Auto,
        refresh: false,
        help: (),
        no_progress: false,
        quiet: 0,
        verbose: 0,
        log_file: None,
        no_log_file: false,
        version: (),
        show_settings: true,
    }
    AutoUpdateSettings {
        cooldown_days: 0,
    }

    ----- stderr -----
    ");
}

#[test]
fn global_config_applies_cooldown_days() {
    let context = TestContext::new();
    context.write_user_config(indoc::indoc! {r"
        [auto_update]
        cooldown_days = 3
    "});

    cmd_snapshot!(context.filters(), context.auto_update().arg("--show-settings"), @"
    success: true
    exit_code: 0
    ----- stdout -----
    GlobalArgs {
        config: None,
        cd: None,
        color: Auto,
        refresh: false,
        help: (),
        no_progress: false,
        quiet: 0,
        verbose: 0,
        log_file: None,
        no_log_file: false,
        version: (),
        show_settings: true,
    }
    AutoUpdateSettings {
        cooldown_days: 3,
    }

    ----- stderr -----
    ");
}

#[test]
fn global_config_cli_args_override_file() {
    let context = TestContext::new();
    context.write_user_config(indoc::indoc! {r"
        [auto_update]
        cooldown_days = 3
    "});

    cmd_snapshot!(
        context.filters(),
        context
            .auto_update()
            .arg("--show-settings")
            .arg("--cooldown-days")
            .arg("0"),
        @"
    success: true
    exit_code: 0
    ----- stdout -----
    GlobalArgs {
        config: None,
        cd: None,
        color: Auto,
        refresh: false,
        help: (),
        no_progress: false,
        quiet: 0,
        verbose: 0,
        log_file: None,
        no_log_file: false,
        version: (),
        show_settings: true,
    }
    AutoUpdateSettings {
        cooldown_days: 0,
    }

    ----- stderr -----
    ");
}

#[test]
fn global_config_ignores_unknown_options() {
    let context = TestContext::new();
    context.write_user_config(indoc::indoc! {r#"
        future_option = true

        [auto_update]
        cooldown_days = 3
        future_option = "ignored"
    "#});

    cmd_snapshot!(context.filters(), context.auto_update().arg("--show-settings"), @"
    success: true
    exit_code: 0
    ----- stdout -----
    GlobalArgs {
        config: None,
        cd: None,
        color: Auto,
        refresh: false,
        help: (),
        no_progress: false,
        quiet: 0,
        verbose: 0,
        log_file: None,
        no_log_file: false,
        version: (),
        show_settings: true,
    }
    AutoUpdateSettings {
        cooldown_days: 3,
    }

    ----- stderr -----
    ");
}

#[test]
fn global_config_invalid_file_reports_parse_error() {
    let context = TestContext::new();
    context.write_user_config(indoc::indoc! {r#"
        [auto_update]
        cooldown_days = "soon"
    "#});

    cmd_snapshot!(context.filters(), context.auto_update().arg("--show-settings"), @r#"
    success: false
    exit_code: 2
    ----- stdout -----
    GlobalArgs {
        config: None,
        cd: None,
        color: Auto,
        refresh: false,
        help: (),
        no_progress: false,
        quiet: 0,
        verbose: 0,
        log_file: None,
        no_log_file: false,
        version: (),
        show_settings: true,
    }

    ----- stderr -----
    error: Failed to parse global config `[HOME]/config/prek/prek.toml`
      caused by: TOML parse error at line 2, column 17
      |
    2 | cooldown_days = "soon"
      |                 ^^^^^^
    invalid type: string "soon", expected u8
    "#);
}
