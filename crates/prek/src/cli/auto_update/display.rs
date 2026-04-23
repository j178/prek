use std::fmt::Write;

use annotate_snippets::{AnnotationKind, Level, Renderer, Snippet, renderer::DecorStyle};
use anyhow::Result;
use itertools::Itertools;
use owo_colors::OwoColorize;

use crate::cli::auto_update::{
    ApplyRepoUpdatesResult, CommitPresence, DisplayEvent, DisplayEventKind, DisplayStream,
    FrozenMismatch, FrozenMismatchAction, FrozenMismatchReason, FrozenWarningEvent, ProjectUpdates,
    RepoOccurrences, RepoUpdate, Revision,
};
use crate::fs::Simplified;
use crate::printer::Printer;

/// Emits all frozen-comment warnings before the normal update output.
pub(super) fn warn_frozen_mismatches(updates: &[RepoUpdate<'_>], printer: Printer) -> Result<()> {
    let mut warnings = Vec::new();

    for update in updates {
        let Ok(resolved) = &update.result else {
            continue;
        };

        for mismatch in &resolved.frozen_mismatches {
            warnings.push(FrozenWarningEvent {
                project: mismatch.project,
                repo: update.target.repo,
                current_rev: update.target.current_rev,
                remote_index: mismatch.remote_index,
                mismatch,
            });
        }
    }

    warnings.sort_by(|a, b| {
        a.project
            .idx()
            .cmp(&b.project.idx())
            .then_with(|| a.remote_index.cmp(&b.remote_index))
    });

    for warning in warnings {
        write!(
            printer.stderr(),
            "{}",
            render_frozen_mismatch_warning(warning.repo, warning.current_rev, warning.mismatch)
        )?;
    }

    Ok(())
}

fn update_verb(dry_run: bool) -> &'static str {
    if dry_run { "would update" } else { "updating" }
}

fn remove_verb(dry_run: bool) -> &'static str {
    if dry_run { "would remove" } else { "removing" }
}

fn format_revision(rev: &str, frozen: Option<&str>) -> String {
    match frozen {
        Some(frozen) => format!(
            "`{}` {}",
            rev.cyan(),
            format!("(frozen: {frozen})").dimmed()
        ),
        None => format!("`{}`", rev.cyan()),
    }
}

fn format_display_event(kind: &DisplayEventKind, dry_run: bool) -> String {
    match kind {
        DisplayEventKind::Update { current, next } => format!(
            "{} {} -> {}",
            format!("{} rev", update_verb(dry_run)).green(),
            format_revision(&current.rev, current.frozen.as_deref()),
            format_revision(&next.rev, next.frozen.as_deref())
        ),
        DisplayEventKind::FrozenUpdate { current, next } => format!(
            "{} `{}` -> `{}`",
            format!("{} frozen comment", update_verb(dry_run)).green(),
            current.cyan(),
            next.cyan()
        ),
        DisplayEventKind::FrozenRemove { current } => format!(
            "{} `{}`",
            format!("{} frozen comment", remove_verb(dry_run)).yellow(),
            current.cyan()
        ),
        DisplayEventKind::UpToDate { current } => format!(
            "{} {}",
            "already up to date at".dimmed(),
            format_revision(&current.rev, current.frozen.as_deref())
        ),
        DisplayEventKind::Failure { error } => {
            format!("{} {error}", "update failed:".red())
        }
    }
}

fn write_display_events(
    events: &mut [DisplayEvent<'_>],
    repo_occurrences: &RepoOccurrences<'_>,
    dry_run: bool,
    printer: Printer,
) -> Result<()> {
    if events.is_empty() {
        return Ok(());
    }

    events.sort_by(|a, b| {
        a.project
            .idx()
            .cmp(&b.project.idx())
            .then_with(|| a.remote_index.cmp(&b.remote_index))
    });

    for stream in [DisplayStream::Stdout, DisplayStream::Stderr] {
        let stream_events = events
            .iter()
            .filter(|event| event.stream == stream)
            .collect::<Vec<_>>();
        if stream_events.is_empty() {
            continue;
        }

        let show_project_headers = stream_events
            .iter()
            .map(|event| event.project.config_file())
            .unique()
            .count()
            > 1;

        let mut current_project = None;
        let mut current_repo = None;
        let mut output = String::new();

        for event in stream_events {
            let project = event.project.config_file();
            if show_project_headers && current_project != Some(project) {
                if current_project.is_some() {
                    writeln!(output)?;
                }
                writeln!(
                    output,
                    "{}",
                    format!("{}", project.user_display()).yellow().bold()
                )?;
                current_project = Some(project);
                current_repo = None;
            }

            let repo_key = (project, event.repo);
            if current_repo != Some(repo_key) {
                if current_repo.is_some() {
                    writeln!(output)?;
                }
                let indent = if show_project_headers { "  " } else { "" };
                writeln!(output, "{}{}", indent, event.repo.cyan().bold())?;
                current_repo = Some(repo_key);
            }

            let indent = if show_project_headers { "    " } else { "  " };
            let line_prefix = if repo_occurrences[&repo_key] > 1 {
                format!("{} ", format!("line {}:", event.line_number).dimmed())
            } else {
                String::new()
            };
            writeln!(
                output,
                "{}{}{}",
                indent,
                line_prefix,
                format_display_event(&event.kind, dry_run)
            )?;
        }

        match stream {
            DisplayStream::Stdout => write!(printer.stdout(), "{output}")?,
            DisplayStream::Stderr => write!(printer.stderr(), "{output}")?,
        }
    }

    Ok(())
}

/// Applies evaluated repo outcomes, recording config changes and printing stdout/stderr output.
pub(super) fn apply_repo_updates<'a>(
    updates: Vec<RepoUpdate<'a>>,
    verbose: bool,
    dry_run: bool,
    printer: Printer,
    project_updates: &mut ProjectUpdates<'a>,
) -> Result<ApplyRepoUpdatesResult> {
    let mut failure = false;
    let mut has_updates = false;
    let mut display_events = Vec::new();

    for update in updates {
        match update.result {
            Ok(resolved) => {
                let is_changed = update.target.current_rev != resolved.revision.rev;
                let has_frozen_updates = resolved.frozen_mismatches.iter().any(|mismatch| {
                    !matches!(mismatch.action, FrozenMismatchAction::NoReplacement)
                });
                let has_frozen_notice = !resolved.frozen_mismatches.is_empty();

                has_updates |= is_changed || has_frozen_updates;

                if is_changed {
                    for usage in &update.target.usages {
                        display_events.push(DisplayEvent {
                            stream: DisplayStream::Stdout,
                            project: usage.project,
                            repo: update.target.repo,
                            remote_index: usage.remote_index,
                            line_number: usage.rev_line_number,
                            kind: DisplayEventKind::Update {
                                current: Revision {
                                    rev: update.target.current_rev.to_string(),
                                    frozen: usage.current_frozen.clone(),
                                },
                                next: resolved.revision.clone(),
                            },
                        });
                        record_project_revision(
                            project_updates,
                            usage.project,
                            usage.remote_count,
                            usage.remote_index,
                            resolved.revision.clone(),
                        );
                    }
                } else {
                    for mismatch in &resolved.frozen_mismatches {
                        match &mismatch.action {
                            FrozenMismatchAction::ReplaceWith(replacement) => {
                                display_events.push(DisplayEvent {
                                    stream: DisplayStream::Stdout,
                                    project: mismatch.project,
                                    repo: update.target.repo,
                                    remote_index: mismatch.remote_index,
                                    line_number: mismatch.rev_line_number,
                                    kind: DisplayEventKind::FrozenUpdate {
                                        current: mismatch.current_frozen.clone(),
                                        next: replacement.clone(),
                                    },
                                });
                                record_project_revision(
                                    project_updates,
                                    mismatch.project,
                                    mismatch.remote_size,
                                    mismatch.remote_index,
                                    Revision {
                                        rev: update.target.current_rev.to_string(),
                                        frozen: Some(replacement.clone()),
                                    },
                                );
                            }
                            FrozenMismatchAction::Remove => {
                                display_events.push(DisplayEvent {
                                    stream: DisplayStream::Stdout,
                                    project: mismatch.project,
                                    repo: update.target.repo,
                                    remote_index: mismatch.remote_index,
                                    line_number: mismatch.rev_line_number,
                                    kind: DisplayEventKind::FrozenRemove {
                                        current: mismatch.current_frozen.clone(),
                                    },
                                });
                                record_project_revision(
                                    project_updates,
                                    mismatch.project,
                                    mismatch.remote_size,
                                    mismatch.remote_index,
                                    Revision {
                                        rev: update.target.current_rev.to_string(),
                                        frozen: None,
                                    },
                                );
                            }
                            FrozenMismatchAction::NoReplacement => {}
                        }
                    }
                }

                if verbose && !is_changed && !has_frozen_notice {
                    for usage in &update.target.usages {
                        display_events.push(DisplayEvent {
                            stream: DisplayStream::Stdout,
                            project: usage.project,
                            repo: update.target.repo,
                            remote_index: usage.remote_index,
                            line_number: usage.rev_line_number,
                            kind: DisplayEventKind::UpToDate {
                                current: Revision {
                                    rev: update.target.current_rev.to_string(),
                                    frozen: usage.current_frozen.clone(),
                                },
                            },
                        });
                    }
                }
            }
            Err(e) => {
                failure = true;
                let error = e.to_string();
                for usage in &update.target.usages {
                    display_events.push(DisplayEvent {
                        stream: DisplayStream::Stderr,
                        project: usage.project,
                        repo: update.target.repo,
                        remote_index: usage.remote_index,
                        line_number: usage.rev_line_number,
                        kind: DisplayEventKind::Failure {
                            error: error.clone(),
                        },
                    });
                }
            }
        }
    }

    let repo_occurrences =
        display_events
            .iter()
            .fold(RepoOccurrences::default(), |mut counts, event| {
                *counts
                    .entry((event.project.config_file(), event.repo))
                    .or_default() += 1;
                counts
            });

    write_display_events(&mut display_events, &repo_occurrences, dry_run, printer)?;

    Ok(ApplyRepoUpdatesResult {
        failure,
        has_updates,
    })
}

fn record_project_revision<'a>(
    project_updates: &mut ProjectUpdates<'a>,
    project: &'a crate::workspace::Project,
    remote_size: usize,
    remote_index: usize,
    revision: Revision,
) {
    let revisions = project_updates
        .entry(project.into())
        .or_insert_with(|| vec![None; remote_size]);
    revisions[remote_index] = Some(revision);
}

/// Formats one stale `# frozen:` warning as an annotated source snippet.
fn render_frozen_mismatch_warning(
    repo: &str,
    current_rev: &str,
    mismatch: &FrozenMismatch<'_>,
) -> String {
    let label = match mismatch.reason {
        FrozenMismatchReason::ResolvesToDifferentCommit => {
            format!(
                "`{}` resolves to a different commit",
                mismatch.current_frozen
            )
        }
        FrozenMismatchReason::Unresolvable => {
            format!("`{}` could not be resolved", mismatch.current_frozen)
        }
    };
    let details = match &mismatch.action {
        FrozenMismatchAction::ReplaceWith(replacement) => Some(format!(
            "pinned commit `{current_rev}` is referenced by `{replacement}`"
        )),
        FrozenMismatchAction::Remove => Some(format!(
            "no tag points at the pinned commit `{current_rev}`"
        )),
        FrozenMismatchAction::NoReplacement
            if matches!(mismatch.current_rev_presence, CommitPresence::Absent) =>
        {
            Some(format!(
                "pinned commit `{current_rev}` is not present in the repo"
            ))
        }
        FrozenMismatchAction::NoReplacement => None,
    };
    let title = format!(
        "[{repo}] frozen ref `{}` does not match `{current_rev}`",
        mismatch.current_frozen
    );

    let site = mismatch
        .frozen_site
        .as_ref()
        .expect("frozen comment site must exist when rendering a frozen mismatch warning");
    let mut report = Level::WARNING.primary_title(title).element(
        Snippet::source(&site.source_line)
            .line_start(site.line_number)
            .path(mismatch.project.config_file().user_display().to_string())
            .annotation(AnnotationKind::Primary.span(site.span.clone()).label(label)),
    );
    if let Some(details) = details {
        report = report.element(Level::NOTE.message(details));
    }

    let renderer = Renderer::styled().decor_style(DecorStyle::Ascii);
    format!("{}\n", renderer.render(&[report]))
}
