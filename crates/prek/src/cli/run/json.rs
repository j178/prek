//! Machine-readable report for `prek run --output-format json`.

use std::fmt::Write as _;

use anyhow::Result;
use serde::Serialize;

use crate::printer::Printer;
use crate::terminal::sanitize_output;

use super::run::{HookRunResult, ModificationScope, ProjectRunResult, RunStatus, SkipReason};

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
enum JsonStatus {
    Passed,
    Failed,
    Skipped,
    DryRun,
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
enum JsonSkipReason {
    Explicit,
    NoFiles,
}

/// Which hooks are responsible for files changed while a priority group ran.
#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
enum JsonModification {
    /// Exactly one hook ran in the group, so it made the changes.
    Hook,
    /// Several hooks ran concurrently, so the changes cannot be attributed to one of them.
    Group,
}

#[derive(Serialize)]
struct JsonHook {
    /// Project path relative to the workspace root. Empty for the root project.
    project: String,
    id: String,
    name: String,
    alias: String,
    status: JsonStatus,
    skip_reason: Option<JsonSkipReason>,
    exit_code: i32,
    duration_secs: f64,
    files_modified: Option<JsonModification>,
    /// Captured output with terminal control sequences resolved and ANSI styles removed. Not parsed.
    output: String,
}

#[derive(Default, Serialize)]
pub(super) struct JsonReport {
    failed: bool,
    hooks: Vec<JsonHook>,
}

impl JsonReport {
    pub(super) fn add(&mut self, project_result: &ProjectRunResult<'_>) {
        let project = project_result.project.relative_path().display().to_string();
        for group in &project_result.groups {
            for result in &group.results {
                self.hooks
                    .push(JsonHook::new(&project, result, group.modification));
            }
        }
    }

    pub(super) fn write(mut self, failed: bool, printer: Printer) -> Result<()> {
        self.failed = failed;
        let json = serde_json::to_string_pretty(&self)?;
        writeln!(printer.stdout_important(), "{json}")?;
        Ok(())
    }
}

impl JsonHook {
    fn new(project: &str, result: &HookRunResult, modification: Option<ModificationScope>) -> Self {
        let (status, skip_reason) = match result.status {
            RunStatus::Passed => (JsonStatus::Passed, None),
            RunStatus::Failed => (JsonStatus::Failed, None),
            RunStatus::DryRun => (JsonStatus::DryRun, None),
            RunStatus::Skipped(SkipReason::Explicit) => {
                (JsonStatus::Skipped, Some(JsonSkipReason::Explicit))
            }
            RunStatus::Skipped(SkipReason::NoFiles) => {
                (JsonStatus::Skipped, Some(JsonSkipReason::NoFiles))
            }
        };

        // Hooks that did not run cannot have modified files.
        let files_modified = if result.status.was_executed() {
            match modification {
                Some(ModificationScope::SingleHook) => Some(JsonModification::Hook),
                Some(ModificationScope::PriorityGroup) => Some(JsonModification::Group),
                None => None,
            }
        } else {
            None
        };

        Self {
            project: project.to_string(),
            id: result.hook.id.clone(),
            name: result.hook.name.clone(),
            alias: result.hook.alias.clone(),
            status,
            skip_reason,
            exit_code: result.exit_status,
            duration_secs: result.duration.as_secs_f64(),
            files_modified,
            output: anstream::adapter::strip_str(&sanitize_output(result.output.trim_ascii()))
                .to_string(),
        }
    }
}
