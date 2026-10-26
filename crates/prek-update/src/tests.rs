use std::process::Command;

use anyhow::{Context, Result};
use prek_consts::env_vars::{EnvVars, EnvVarsRead};

pub(crate) fn is_child() -> bool {
    EnvVars.is_set("PREK_UPDATE_TEST_CHILD")
}

/// Exercise environment-dependent behavior without changing the test runner's environment.
pub(crate) fn child() -> Result<Command> {
    let thread = std::thread::current();
    let name = thread.name().context("Missing test thread name")?;
    let mut command = Command::new(std::env::current_exe()?);
    command
        .args(["--exact", name, "--nocapture"])
        .env("PREK_UPDATE_TEST_CHILD", "1");
    Ok(command)
}

pub(crate) fn check_child(command: &mut Command) -> Result<()> {
    let output = command.output()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stdout}\n{stderr}");
    // A renamed or incorrectly selected child test must not silently run zero tests.
    assert!(stdout.contains("1 passed"), "{stdout}\n{stderr}");
    Ok(())
}
