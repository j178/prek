//! Final hook reports and their coordination with the live progress UI.

use std::fmt::Write as _;
use std::io::Write as _;
use std::path::Path;

use anyhow::{Context, Result};
use owo_colors::{OwoColorize, Styled};
use prek_consts::env_vars::EnvVars;
use unicode_width::UnicodeWidthStr;

use crate::cli::ExitStatus;
use crate::config::HideStatus;
use crate::git;
use crate::printer::Printer;
use crate::terminal::{USE_COLOR, sanitize_output};
use crate::workspace::{Project, Workspace};

use super::run::{
    HookRunResult, ModificationScope, PriorityGroupResult, ProjectRunResult, RunStatus, RunSummary,
    ScheduledHook, SkipReason,
};
use super::{DRY_RUN, FAILED, HookRunReporter, PASSED, SKIPPED, project_status_marker};

#[derive(Clone, Copy, Eq, PartialEq)]
enum ReportTiming {
    AfterGroup,
    AfterLevel,
}

pub(super) struct RunReport {
    pub(super) progress: HookRunReporter,
    status_printer: StatusPrinter,
    printer: Printer,
    verbose: bool,
    show_project_headers: bool,
    timing: ReportTiming,
}

impl RunReport {
    pub(super) fn new(
        hooks: &[ScheduledHook],
        verbose: bool,
        show_project_headers: bool,
        printer: Printer,
    ) -> Self {
        let status_printer = StatusPrinter::for_hooks(hooks, printer);
        let progress =
            HookRunReporter::new(printer, status_printer.bar_len(), show_project_headers);
        // Only a lone root project has no header. Headers include the final
        // project status, so they cannot be printed after an intermediate group.
        let timing = if !show_project_headers && progress.is_hidden() {
            ReportTiming::AfterGroup
        } else {
            ReportTiming::AfterLevel
        };
        Self {
            progress,
            status_printer,
            printer,
            verbose,
            show_project_headers,
            timing,
        }
    }

    pub(super) fn on_project_complete(&self, result: &ProjectRunResult<'_>) {
        if result.has_visible_report() {
            self.progress
                .on_project_complete(result.project, result.failed());
        } else {
            self.progress.hide_project(result.project);
        }
    }

    fn render_project_header(&self, project: &Project, failed: bool) -> Result<()> {
        self.progress.suspend(|| {
            writeln!(
                self.printer.stdout(),
                "{} {}",
                project_status_marker(failed),
                project.display_name().cyan().bold()
            )
        })?;

        Ok(())
    }

    pub(super) fn on_group_complete(
        &self,
        group: &PriorityGroupResult,
        filter: ReportFilter<'_>,
    ) -> Result<()> {
        if self.timing == ReportTiming::AfterGroup {
            self.progress.clear_completed();
            self.render_priority_group(group, "", filter)?;
            anstream::stdout().flush()?;
            return Ok(());
        }

        for result in &group.results {
            let status = result.status;
            match status {
                RunStatus::Passed | RunStatus::Failed if filter.shows(status) => {
                    self.progress
                        .on_run_result(&result.hook, status == RunStatus::Passed);
                }
                RunStatus::Passed | RunStatus::Failed => {
                    self.progress.hide_run_result(&result.hook);
                }
                RunStatus::DryRun | RunStatus::Skipped(_) => {}
            }
        }
        Ok(())
    }

    pub(super) fn write_project(&self, project_result: ProjectRunResult<'_>) -> Result<()> {
        if self.timing == ReportTiming::AfterGroup {
            // Writing hook log files between groups could affect later hooks
            // and their file modification checks.
            for group in &project_result.groups {
                self.write_group_logs(group)?;
            }
            return Ok(());
        }

        let show_project_header = self.show_project_headers && project_result.has_visible_report();
        if show_project_header {
            self.render_project_header(project_result.project, project_result.failed())?;
        }
        let hook_prefix = if show_project_header { "  " } else { "" };

        for group in project_result.groups {
            self.write_group(&group, hook_prefix, project_result.report_filter)?;
        }

        Ok(())
    }

    fn write_group(
        &self,
        group: &PriorityGroupResult,
        hook_prefix: &str,
        filter: ReportFilter<'_>,
    ) -> Result<()> {
        self.progress.clear_completed();
        self.write_group_logs(group)?;
        self.progress
            .suspend(|| self.render_priority_group(group, hook_prefix, filter))?;

        Ok(())
    }

    fn write_group_logs(&self, group: &PriorityGroupResult) -> Result<()> {
        for result in &group.results {
            if result.shows_details(self.verbose) {
                result.write_log_file()?;
            }
        }
        Ok(())
    }

    fn render_priority_group(
        &self,
        group: &PriorityGroupResult,
        hook_prefix: &str,
        filter: ReportFilter<'_>,
    ) -> Result<()> {
        let group_results = &group.results;
        let modifications_belong_to_single_hook =
            group.modification == Some(ModificationScope::SingleHook);
        let show_group_failure = group.shows_group_failure(filter);

        // Hooks that did not run cannot have modified files, so report them outside
        // the modification group.
        let mut visible_results = group_results
            .iter()
            .filter(|result| filter.shows(result.status))
            .filter(|result| !show_group_failure || result.status.was_executed())
            .peekable();
        let mut first_result = true;
        let group_output_prefix = if show_group_failure {
            Some(format!("{hook_prefix}{}", "  │ ".dimmed()))
        } else {
            None
        };
        if show_group_failure {
            self.status_printer.write(
                "Files were modified by following hooks",
                hook_prefix,
                RunStatus::Failed,
            )?;
        }

        while let Some(result) = visible_results.next() {
            let status = result.status;

            let connector = if show_group_failure {
                if visible_results.peek().is_none() {
                    "  └ "
                } else if first_result {
                    "  ┌ "
                } else {
                    "  │ "
                }
            } else {
                ""
            };
            first_result = false;
            let prefix = format!("{hook_prefix}{connector}");
            self.status_printer
                .write(&result.hook.name, &prefix, status)?;

            if !status.is_skipped() && result.shows_details(self.verbose) {
                self.render_hook_details(
                    result,
                    hook_prefix,
                    group_output_prefix.as_deref(),
                    modifications_belong_to_single_hook,
                )?;
            }
        }

        if show_group_failure {
            for result in group_results {
                if !result.status.was_executed() && filter.shows(result.status) {
                    self.status_printer
                        .write(&result.hook.name, hook_prefix, result.status)?;
                }
            }
        }

        Ok(())
    }

    fn render_hook_details(
        &self,
        result: &HookRunResult,
        hook_prefix: &str,
        group_output_prefix: Option<&str>,
        modified_files: bool,
    ) -> Result<()> {
        let detail_prefix = group_output_prefix.unwrap_or(hook_prefix);
        let mut stdout = if result.status.is_failure() {
            self.printer.stdout_important()
        } else {
            self.printer.stdout()
        };

        writeln!(
            stdout,
            "{detail_prefix}{}",
            format!("- hook id: {}", result.hook.id).dimmed()
        )?;
        if !result.hook.alias.is_empty() && result.hook.alias != result.hook.id {
            writeln!(
                stdout,
                "{detail_prefix}{}",
                format!("- hook alias: {}", result.hook.alias).dimmed()
            )?;
        }
        if let Some(description) = result.hook.description.as_deref()
            && let Some(description) = description.trim().lines().next()
        {
            let description = description.trim_end().trim_end_matches('.');
            writeln!(
                stdout,
                "{detail_prefix}{}",
                format!("- description: {description}").dimmed()
            )?;
        }
        if self.verbose || result.hook.verbose {
            writeln!(
                stdout,
                "{detail_prefix}{}",
                format!("- duration: {:.2?}s", result.duration.as_secs_f64()).dimmed()
            )?;
        }
        if result.exit_status != 0 {
            writeln!(
                stdout,
                "{detail_prefix}{}",
                format!("- exit code: {}", result.exit_status).dimmed()
            )?;
        }
        if modified_files {
            writeln!(
                stdout,
                "{detail_prefix}{}",
                "- files were modified by this hook".dimmed()
            )?;
        }

        let output = result.output.trim_ascii();
        if output.is_empty() || result.hook.log_file.is_some() {
            return Ok(());
        }
        let text = sanitize_output(output);
        if text.is_empty() {
            return Ok(());
        }

        let separator = if group_output_prefix.is_some() {
            format!("{hook_prefix}{}", "  │".dimmed())
        } else {
            String::new()
        };
        writeln!(stdout, "{separator}")?;
        for line in text.lines() {
            if line.is_empty() {
                writeln!(stdout, "{separator}")?;
            } else if let Some(group_output_prefix) = group_output_prefix {
                writeln!(stdout, "{group_output_prefix}{line}")?;
            } else {
                writeln!(stdout, "{hook_prefix}  {line}")?;
            }
        }

        Ok(())
    }

    pub(super) async fn finish(
        &self,
        summary: RunSummary,
        workspace: &Workspace,
        show_diff_on_failure: bool,
    ) -> Result<ExitStatus> {
        self.progress.on_complete();

        if summary.failed && show_diff_on_failure && summary.modified_files {
            if EnvVars::is_under_ci() {
                writeln!(
                    self.printer.stdout(),
                    "{}",
                    indoc::formatdoc! {
                        "\n{}: Some hooks made changes to the files.
                        If you are seeing this message in CI, reproduce locally with: `{}`
                        To run prek as part of Git workflow, use `{}` to set up Git shims.\n",
                        "hint".yellow().bold(),
                        "prek run --all-files".cyan(),
                        "prek install".cyan()
                    }
                )?;
            }

            writeln!(
                self.printer.stdout_important(),
                "All changes made by hooks:"
            )?;

            let color = if *USE_COLOR {
                "--color=always"
            } else {
                "--color=never"
            };
            git::git_cmd()?
                .current_dir(workspace.root())
                .arg("--no-pager")
                .arg("diff")
                .hidden_args(["--no-ext-diff"])
                .arg(color)
                .arg("--")
                .arg(workspace.root())
                .check(true)
                .spawn()?
                .wait()
                .await?;
        }

        if summary.failed {
            Ok(ExitStatus::Failure)
        } else {
            Ok(ExitStatus::Success)
        }
    }
}

impl ProjectRunResult<'_> {
    fn has_visible_report(&self) -> bool {
        self.groups
            .iter()
            .any(|group| group.has_visible_report(self.report_filter))
    }
}

impl PriorityGroupResult {
    fn shows_group_failure(&self, filter: ReportFilter<'_>) -> bool {
        self.modification == Some(ModificationScope::PriorityGroup)
            && filter.shows(RunStatus::Failed)
    }

    fn has_visible_report(&self, filter: ReportFilter<'_>) -> bool {
        self.shows_group_failure(filter)
            || self
                .results
                .iter()
                .any(|result| filter.shows(result.status))
    }
}
#[derive(Clone, Copy)]
pub(super) struct ReportFilter<'a> {
    hidden_statuses: &'a [HideStatus],
}

impl<'a> ReportFilter<'a> {
    pub(super) fn new(hidden_statuses: &'a [HideStatus]) -> Self {
        Self { hidden_statuses }
    }

    fn shows(self, status: RunStatus) -> bool {
        let Some(hide_status) = status.hide_status() else {
            return true;
        };
        !self.hidden_statuses.contains(&hide_status)
    }
}

impl RunStatus {
    fn hide_status(self) -> Option<HideStatus> {
        match self {
            Self::Passed => Some(HideStatus::Passed),
            Self::Failed => Some(HideStatus::Failed),
            Self::Skipped(_) => Some(HideStatus::Skipped),
            Self::DryRun => None,
        }
    }

    fn label(self) -> Styled<&'static str> {
        match self {
            Self::Passed => PASSED,
            Self::Failed => FAILED,
            Self::DryRun => DRY_RUN,
            Self::Skipped(_) => SKIPPED,
        }
    }

    fn suffix(self) -> &'static str {
        match self {
            Self::Skipped(SkipReason::NoFiles) => "(no files to check)",
            Self::Passed | Self::Failed | Self::DryRun | Self::Skipped(SkipReason::Explicit) => "",
        }
    }
}

struct StatusPrinter {
    printer: Printer,
    columns: usize,
}

impl StatusPrinter {
    fn for_hooks(hooks: &[ScheduledHook], printer: Printer) -> Self {
        let name_len = hooks
            .iter()
            .map(|hook| hook.name.width())
            .max()
            .unwrap_or(0);
        let widest_status = RunStatus::Skipped(SkipReason::NoFiles);
        let status_width = widest_status.suffix().width() + widest_status.label().inner().width();
        let columns = std::cmp::max(
            79,
            // Hook name...(no files to check)Skipped
            name_len + 3 + status_width,
        );
        Self { printer, columns }
    }

    fn bar_len(&self) -> usize {
        self.columns - RunStatus::Passed.label().inner().width()
    }

    fn write(
        &self,
        hook_name: &str,
        prefix: &str,
        status: RunStatus,
    ) -> Result<(), std::fmt::Error> {
        let suffix = status.suffix();
        let status_line = status.label();
        let (prefix, prefix_width) = if prefix.is_empty() {
            (String::new(), 0)
        } else {
            (prefix.dimmed().to_string(), prefix.width())
        };
        let used_width =
            prefix_width + hook_name.width() + suffix.width() + status_line.inner().width();
        let dots = self.columns.saturating_sub(used_width);
        let dots = ".".repeat(dots).green().to_string();
        let line = format!("{prefix}{hook_name}{dots}{suffix}{status_line}");
        if status.is_failure() {
            writeln!(self.printer.stdout_important(), "{line}")
        } else {
            writeln!(self.printer.stdout(), "{line}")
        }
    }
}

impl HookRunResult {
    fn shows_details(&self, verbose: bool) -> bool {
        verbose || self.hook.verbose || self.status.is_failure()
    }

    fn write_log_file(&self) -> Result<()> {
        let Some(file) = self.hook.log_file.as_deref() else {
            return Ok(());
        };
        let output = self.output.trim_ascii();
        if output.is_empty() {
            return Ok(());
        }

        let file = Path::new(file);
        let config_file = self.hook.project().config_file();
        let file = if file.is_relative() {
            let config_dir = config_file
                .parent()
                .context("Configuration file must have a parent directory")?;
            config_dir.join(file)
        } else {
            file.to_path_buf()
        };
        let mut file = fs_err::OpenOptions::new()
            .create(true)
            .append(true)
            .open(file)?;
        file.write_all(output)?;
        file.flush()?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_printer_write_dots_saturates_instead_of_underflow() {
        let status_printer = StatusPrinter {
            printer: Printer::Silent,
            columns: 10,
        };

        // This would underflow if computed with plain `-` on `usize`.
        let long_name = "this hook name is definitely longer than ten columns";
        status_printer
            .write(long_name, "", RunStatus::Failed)
            .expect("write should not fail");
    }
}
