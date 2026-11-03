mod common;

use prek_consts::env_vars::EnvVars;

use common::{TestEnv, cmd_snapshot};

fn download_context() -> TestEnv {
    TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: deno
                name: deno
                language: deno
                entry: deno --version
                always_run: true
    "})
        .init_git()
}

/// Invalid custom CA data must produce a normal CLI error with its cause, even
/// when system certificates are enabled, without suggesting a system CA fix.
#[test]
fn invalid_custom_certificate() {
    let context = download_context().with_file(
        "invalid.pem",
        "-----BEGIN CERTIFICATE-----\nAA==\n-----END CERTIFICATE-----\n",
    );

    let mut command = context.run();
    command
        .env(
            EnvVars::PREK_INTERNAL__DENO_BINARY_NAME,
            "deno-not-installed",
        )
        .env(EnvVars::PREK_SYSTEM_CERTS, "true")
        .env(EnvVars::SSL_CERT_FILE, context.child("invalid.pem").path())
        .env_remove(EnvVars::SSL_CERT_DIR);

    cmd_snapshot!(context, command, @"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Failed to install hook `deno`
      caused by: Failed to install deno
      caused by: Failed to list remote versions
      caused by: Failed to initialize HTTP client
      caused by: builder error
      caused by: invalid peer certificate: BadEncoding
    ");
}
