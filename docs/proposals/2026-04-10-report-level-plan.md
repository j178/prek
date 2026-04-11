# `--report-level` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a `--report-level <level>` flag to `prek run` that filters which hook status lines are displayed, without changing execution semantics.

**Architecture:** Two-pass approach — execution is unchanged, report level gates rendering. Selector-skipped hooks are tracked separately and rendered after executed hooks per project. A `ReportLevel` enum with 6 ordered variants implements threshold-based filtering via a `should_show(RunStatus) -> bool` method.

**Tech Stack:** Rust, clap (CLI parsing with `ValueEnum` derive), existing snapshot test infrastructure (`insta`, `assert_cmd`)

**Spec:** `docs/proposals/2026-04-10-report-level-design.md`

---

### Task 1: Add `PREK_REPORT_LEVEL` environment variable constant

**Files:**
- Modify: `crates/prek-consts/src/env_vars.rs:20-34`

- [ ] **Step 1: Add the constant**

In `crates/prek-consts/src/env_vars.rs`, add after the `PREK_QUIET` line (line 33):

```rust
    pub const PREK_REPORT_LEVEL: &'static str = "PREK_REPORT_LEVEL";
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check -p prek-consts`
Expected: compiles with no errors

- [ ] **Step 3: Commit**

```bash
git add crates/prek-consts/src/env_vars.rs
git commit -m "feat: add PREK_REPORT_LEVEL env var constant"
```

---

### Task 2: Define `ReportLevel` enum and add CLI flag

**Files:**
- Modify: `crates/prek/src/cli/mod.rs:436-542`

- [ ] **Step 1: Define the `ReportLevel` enum**

Add the enum definition after the existing `RunArgs` struct (after line 542) in `crates/prek/src/cli/mod.rs`. Follow the pattern used by `ColorChoice` (line 90) and `ListOutputFormat` (line 557):

```rust
/// Controls which hook status lines are displayed during a run.
///
/// Levels are ordered from least to most verbose. Each level includes
/// all statuses from lower levels plus its own.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, clap::ValueEnum)]
pub(crate) enum ReportLevel {
    /// Show no per-hook status lines.
    Silent,
    /// Show only failed hooks.
    Fail,
    /// Show failed hooks and hooks skipped because no files matched or language is unimplemented.
    SkippedNoFiles,
    /// Show failed, no-files, and hooks excluded by --skip/SKIP/PREK_SKIP.
    Skipped,
    /// Show failed, skipped, and passed hooks (including dry-run).
    #[default]
    Passed,
    /// Show every hook status.
    All,
}
```

- [ ] **Step 2: Add the `report_level` field to `RunArgs`**

In `crates/prek/src/cli/mod.rs`, add a new field to the `RunArgs` struct (after the `dry_run` field, around line 538):

```rust
    /// Control which hook statuses are shown in output.
    ///
    /// Levels from least to most verbose: silent, fail, skipped-no-files,
    /// skipped, passed, all. Each level includes all statuses from lower levels.
    #[arg(
        long,
        value_enum,
        env = EnvVars::PREK_REPORT_LEVEL,
        default_value_t = ReportLevel::Passed,
    )]
    pub(crate) report_level: ReportLevel,
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo check -p prek`
Expected: compiles with no errors (the field is defined but not yet used)

- [ ] **Step 4: Commit**

```bash
git add crates/prek/src/cli/mod.rs
git commit -m "feat: define ReportLevel enum and --report-level CLI flag"
```

---

### Task 3: Add `RunStatus::Skipped` variant and `ReportLevel::should_show`

**Files:**
- Modify: `crates/prek/src/cli/run/run.rs:954-978`

- [ ] **Step 1: Add `Skipped` variant to `RunStatus`**

In `crates/prek/src/cli/run/run.rs`, add the `Skipped` variant to the `RunStatus` enum (line 954-961):

```rust
#[derive(Copy, Clone, Eq, PartialEq)]
enum RunStatus {
    Success,
    Failed,
    DryRun,
    NoFiles,
    Unimplemented,
    Skipped,
}
```

- [ ] **Step 2: Update `RunStatus` methods to handle `Skipped`**

Update the three methods on `RunStatus` (lines 963-978):

```rust
impl RunStatus {
    fn as_bool(self) -> bool {
        matches!(
            self,
            Self::Success | Self::NoFiles | Self::DryRun | Self::Unimplemented | Self::Skipped
        )
    }

    fn is_unimplemented(self) -> bool {
        matches!(self, Self::Unimplemented)
    }

    fn is_skipped(self) -> bool {
        matches!(self, Self::DryRun | Self::NoFiles | Self::Unimplemented | Self::Skipped)
    }
}
```

- [ ] **Step 3: Add `should_show` method to `ReportLevel`**

Import `ReportLevel` at the top of `crates/prek/src/cli/run/run.rs` and add this `impl` block. Place it right after the `ReportLevel` import or near the `RunStatus` impl:

```rust
use crate::cli::ReportLevel;
```

Then add the impl (near the `RunStatus` impl, around line 978):

```rust
impl ReportLevel {
    fn should_show(self, status: RunStatus) -> bool {
        match status {
            RunStatus::Failed => self >= ReportLevel::Fail,
            RunStatus::NoFiles | RunStatus::Unimplemented => self >= ReportLevel::SkippedNoFiles,
            RunStatus::Skipped => self >= ReportLevel::Skipped,
            RunStatus::Success | RunStatus::DryRun => self >= ReportLevel::Passed,
        }
    }
}
```

- [ ] **Step 4: Add `Skipped` rendering to `StatusPrinter::write`**

In `StatusPrinter::write` (line 524-570), add a constant and a match arm. Add the constant with the others (around line 499):

```rust
    const EXCLUDED: &'static str = "(excluded by skip)";
```

Add the match arm in the `write` method's match block (after the `Unimplemented` arm, around line 540):

```rust
            RunStatus::Skipped => (
                Self::EXCLUDED,
                Self::SKIPPED.black().on_yellow().to_string(),
                Self::SKIPPED.width(),
            ),
```

- [ ] **Step 5: Verify it compiles**

Run: `cargo check -p prek`
Expected: compiles (warnings about unused `ReportLevel::should_show` are fine — it's used in the next task)

- [ ] **Step 6: Commit**

```bash
git add crates/prek/src/cli/run/run.rs
git commit -m "feat: add RunStatus::Skipped variant and ReportLevel::should_show"
```

---

### Task 4: Thread `report_level` through the call chain

**Files:**
- Modify: `crates/prek/src/main.rs:276-298`
- Modify: `crates/prek/src/cli/run/run.rs:34-54,203-213,575-584`

- [ ] **Step 1: Pass `report_level` from main.rs**

In `crates/prek/src/main.rs`, add `args.report_level` to the `cli::run()` call (around line 279-298). Add it after `args.extra` (line 295):

```rust
        Command::Run(args) => {
            show_settings!(args);

            cli::run(
                &store,
                cli.globals.config,
                args.includes,
                args.skips,
                args.stage,
                args.from_ref,
                args.to_ref,
                args.all_files,
                args.files,
                args.directory,
                args.last_commit,
                args.show_diff_on_failure,
                flag(args.fail_fast, args.no_fail_fast),
                args.dry_run,
                cli.globals.refresh,
                args.extra,
                args.report_level,
                cli.globals.verbose > 0,
                printer,
            )
            .await
        }
```

- [ ] **Step 2: Add `report_level` parameter to `cli::run()`**

In `crates/prek/src/cli/run/run.rs`, update the `run()` function signature (lines 34-54). Add `report_level: ReportLevel` after `extra_args: RunExtraArgs`:

```rust
pub(crate) async fn run(
    store: &Store,
    config: Option<PathBuf>,
    includes: Vec<String>,
    skips: Vec<String>,
    hook_stage: Option<Stage>,
    from_ref: Option<String>,
    to_ref: Option<String>,
    all_files: bool,
    files: Vec<String>,
    directories: Vec<String>,
    last_commit: bool,
    show_diff_on_failure: bool,
    fail_fast: Option<bool>,
    dry_run: bool,
    refresh: bool,
    extra_args: RunExtraArgs,
    report_level: ReportLevel,
    verbose: bool,
    printer: Printer,
) -> Result<ExitStatus> {
```

- [ ] **Step 3: Pass `report_level` to `run_hooks()`**

Update the `run_hooks()` call site (around line 203-213):

```rust
    run_hooks(
        &workspace,
        &installed_hooks,
        filenames,
        store,
        show_diff_on_failure,
        fail_fast,
        dry_run,
        report_level,
        verbose,
        printer,
    )
    .await
```

- [ ] **Step 4: Add `report_level` parameter to `run_hooks()`**

Update the `run_hooks()` function signature (lines 575-584):

```rust
async fn run_hooks(
    workspace: &Workspace,
    hooks: &[InstalledHook],
    filenames: Vec<PathBuf>,
    store: &Store,
    show_diff_on_failure: bool,
    fail_fast: Option<bool>,
    dry_run: bool,
    report_level: ReportLevel,
    verbose: bool,
    printer: Printer,
) -> Result<ExitStatus> {
```

- [ ] **Step 5: Verify it compiles**

Run: `cargo check -p prek`
Expected: compiles with no errors

- [ ] **Step 6: Commit**

```bash
git add crates/prek/src/main.rs crates/prek/src/cli/run/run.rs
git commit -m "feat: thread report_level through run call chain"
```

---

### Task 5: Filter rendering by report level

**Files:**
- Modify: `crates/prek/src/cli/run/run.rs:626-672,797-917`

- [ ] **Step 1: Pass `report_level` to `render_priority_group()`**

Update the call site in `run_hooks()` (around line 664-672):

```rust
            reporter.suspend(|| {
                render_priority_group(
                    printer,
                    &status_printer,
                    &group_results,
                    verbose,
                    group_modified_files,
                    report_level,
                )
            })?;
```

- [ ] **Step 2: Update `render_priority_group` signature and add filtering**

Update `render_priority_group` (line 797) to accept `report_level` and gate status line output:

```rust
fn render_priority_group(
    printer: Printer,
    status_printer: &StatusPrinter,
    group_results: &[RunResult],
    verbose: bool,
    group_modified_files: bool,
    report_level: ReportLevel,
) -> Result<()> {
    // Only show a special group UI when the group failed due to file modifications
    // AND at least one hook in the group is visible at the current report level.
    let any_visible = group_results
        .iter()
        .any(|r| report_level.should_show(r.status));

    let show_group_ui =
        group_modified_files && group_results.len() > 1 && any_visible;
    let single_hook_modified_files = group_results.len() == 1 && group_modified_files;
    let group_prefix = if show_group_ui {
        format!("{}", "  │ ".dimmed())
    } else {
        String::new()
    };

    if show_group_ui {
        status_printer.write(
            "Files were modified by following hooks",
            "",
            RunStatus::Failed,
        )?;
    }

    for (i, result) in group_results.iter().enumerate() {
        let prefix = if show_group_ui {
            if i == 0 {
                "  ┌ "
            } else if i + 1 == group_results.len() {
                "  └ "
            } else {
                "  │ "
            }
        } else {
            ""
        };

        // If a single hook modified files, treat it as failed.
        let status = if single_hook_modified_files && result.status == RunStatus::Success {
            RunStatus::Failed
        } else {
            result.status
        };

        // Skip rendering if below the report level threshold.
        if !report_level.should_show(status) {
            continue;
        }

        status_printer.write(&result.hook.name, prefix, status)?;

        if matches!(status, RunStatus::NoFiles | RunStatus::Unimplemented) {
            continue;
        }

        let mut stdout = match status {
            RunStatus::Failed => printer.stdout_important(),
            _ => printer.stdout(),
        };

        if verbose || result.hook.verbose || status == RunStatus::Failed {
            writeln!(
                stdout,
                "{group_prefix}{}",
                format!("- hook id: {}", result.hook.id).dimmed()
            )?;
            if verbose || result.hook.verbose {
                writeln!(
                    stdout,
                    "{group_prefix}{}",
                    format!("- duration: {:.2?}s", result.duration.as_secs_f64()).dimmed()
                )?;
            }
            if result.exit_status != 0 {
                writeln!(
                    stdout,
                    "{group_prefix}{}",
                    format!("- exit code: {}", result.exit_status).dimmed()
                )?;
            }
            if single_hook_modified_files {
                writeln!(
                    stdout,
                    "{group_prefix}{}",
                    "- files were modified by this hook".dimmed()
                )?;
            }

            let output = result.output.trim_ascii();
            if !output.is_empty() {
                if let Some(file) = result.hook.log_file.as_deref() {
                    let mut file = fs_err::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(file)?;
                    file.write_all(output)?;
                    file.flush()?;
                } else {
                    if show_group_ui {
                        writeln!(stdout, "{}", "  │".dimmed())?;
                    } else {
                        writeln!(stdout)?;
                    }
                    let text = String::from_utf8_lossy(output);
                    for line in text.lines() {
                        if line.is_empty() {
                            if show_group_ui {
                                writeln!(stdout, "{}", "  │".dimmed())?;
                            } else {
                                writeln!(stdout)?;
                            }
                        } else {
                            if show_group_ui {
                                writeln!(stdout, "{group_prefix}{line}")?;
                            } else {
                                writeln!(stdout, "  {line}")?;
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(())
}
```

- [ ] **Step 3: Suppress project headers when `report_level` is `Silent`**

In `run_hooks()`, wrap the project header output (around line 626-636) with a report level check:

```rust
        if (projects_len > 1 || !project.is_root())
            && report_level != ReportLevel::Silent
        {
            reporter.suspend(|| {
                writeln!(
                    status_printer.printer().stdout(),
                    "{}{}",
                    if first { "" } else { "\n" },
                    format!("Running hooks for `{}`:", project.to_string().cyan()).bold()
                )
            })?;
            first = false;
        }
```

- [ ] **Step 4: Verify it compiles**

Run: `cargo check -p prek`
Expected: compiles with no errors

- [ ] **Step 5: Verify existing tests still pass**

Run: `cargo nextest run -p prek --test skipped_hooks`
Expected: all existing tests pass (default `passed` level matches current behavior)

- [ ] **Step 6: Commit**

```bash
git add crates/prek/src/cli/run/run.rs
git commit -m "feat: filter hook status rendering by report level"
```

---

### Task 6: Track and render selector-skipped hooks

**Files:**
- Modify: `crates/prek/src/cli/run/run.rs:92-100,575-686`

- [ ] **Step 1: Partition hooks into selected and skipped**

In `crates/prek/src/cli/run/run.rs`, replace the filter at lines 92-100 with a partition:

```rust
    let hooks = workspace
        .init_hooks(store, Some(&reporter))
        .await
        .context("Failed to init hooks")?;

    let mut selected_hooks = Vec::new();
    let mut skipped_by_selector: Vec<Arc<Hook>> = Vec::new();
    for hook in hooks {
        if selectors.matches_hook(&hook) {
            selected_hooks.push(Arc::new(hook));
        } else {
            skipped_by_selector.push(Arc::new(hook));
        }
    }
```

- [ ] **Step 2: Filter skipped hooks by the active stage**

After the stage resolution logic (around line 148), filter `skipped_by_selector` to only include hooks for the active stage. This prevents showing skipped hooks from unrelated stages (e.g., `pre-push` hooks when running `pre-commit`):

```rust
    let skipped_by_selector: Vec<_> = skipped_by_selector
        .into_iter()
        .filter(|h| h.stages.contains(hook_stage))
        .collect();
```

- [ ] **Step 3: Pass `skipped_by_selector` to `run_hooks()`**

Update the `run_hooks()` call (around line 203) to include `skipped_by_selector`:

```rust
    run_hooks(
        &workspace,
        &installed_hooks,
        &skipped_by_selector,
        filenames,
        store,
        show_diff_on_failure,
        fail_fast,
        dry_run,
        report_level,
        verbose,
        printer,
    )
    .await
```

Update the `run_hooks()` function signature:

```rust
async fn run_hooks(
    workspace: &Workspace,
    hooks: &[InstalledHook],
    skipped_by_selector: &[Arc<Hook>],
    filenames: Vec<PathBuf>,
    store: &Store,
    show_diff_on_failure: bool,
    fail_fast: Option<bool>,
    dry_run: bool,
    report_level: ReportLevel,
    verbose: bool,
    printer: Printer,
) -> Result<ExitStatus> {
```

- [ ] **Step 4: Include skipped hook names in `StatusPrinter` column width**

In `run_hooks()`, update the `StatusPrinter` construction (around line 588) to account for skipped hook names:

```rust
    let status_printer = StatusPrinter::for_hooks_with_skipped(hooks, skipped_by_selector, printer);
```

Add a new constructor method to `StatusPrinter` (near line 502):

```rust
    fn for_hooks_with_skipped(
        hooks: &[InstalledHook],
        skipped: &[Arc<Hook>],
        printer: Printer,
    ) -> Self {
        let name_len = hooks
            .iter()
            .map(|hook| hook.name.width())
            .chain(skipped.iter().map(|hook| hook.name.width()))
            .max()
            .unwrap_or(0);
        let columns = std::cmp::max(
            79,
            name_len + 3 + Self::NO_FILES.len() + Self::SKIPPED.len(),
        );
        Self { printer, columns }
    }
```

- [ ] **Step 5: Render selector-skipped hooks after executed hooks per project**

In `run_hooks()`, after each project's priority group loop (after line 684, before the closing of the `'outer` loop), add rendering for selector-skipped hooks belonging to this project:

```rust
            // Render selector-skipped hooks for this project, after executed hooks.
            if report_level.should_show(RunStatus::Skipped) {
                let mut project_skipped: Vec<_> = skipped_by_selector
                    .iter()
                    .filter(|h| h.project() == project)
                    .collect();
                project_skipped.sort_by_key(|h| h.idx);

                for hook in project_skipped {
                    reporter.suspend(|| {
                        status_printer.write(&hook.name, "", RunStatus::Skipped)
                    })?;
                }
            }
```

- [ ] **Step 6: Verify it compiles**

Run: `cargo check -p prek`
Expected: compiles with no errors

- [ ] **Step 7: Commit**

```bash
git add crates/prek/src/cli/run/run.rs
git commit -m "feat: track and render selector-skipped hooks"
```

---

### Task 7: Write integration tests

**Files:**
- Modify: `crates/prek/tests/skipped_hooks.rs`

- [ ] **Step 1: Add test for `--report-level fail`**

Add to `crates/prek/tests/skipped_hooks.rs`:

```rust
/// `--report-level fail` hides passed and no-files hooks, shows only failures.
#[test]
fn report_level_fail_shows_only_failures() -> Result<()> {
    let context = TestContext::new();
    context.init_project();

    let cwd = context.work_dir();

    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: pass-hook
                name: pass-hook
                language: system
                entry: echo "ok"
                files: \.txt$
              - id: fail-hook
                name: fail-hook
                language: system
                entry: exit 1
                files: \.txt$
              - id: no-files-hook
                name: no-files-hook
                language: system
                entry: echo "checking"
                files: \.py$
    "#});

    cwd.child("file.txt").write_str("content")?;
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run().arg("--report-level").arg("fail"), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    fail-hook...............................................................Failed
    - hook id: fail-hook
    - exit code: 1

    ----- stderr -----
    "#);

    Ok(())
}
```

- [ ] **Step 2: Run the test to verify**

Run: `cargo nextest run -p prek --test skipped_hooks report_level_fail_shows_only_failures`
Expected: PASS (or snapshot needs updating with `cargo insta review`)

- [ ] **Step 3: Add test for `--report-level silent`**

```rust
/// `--report-level silent` shows no per-hook status lines but still fails.
#[test]
fn report_level_silent_no_output_but_exit_code() -> Result<()> {
    let context = TestContext::new();
    context.init_project();

    let cwd = context.work_dir();

    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: fail-hook
                name: fail-hook
                language: system
                entry: exit 1
                files: \.txt$
    "#});

    cwd.child("file.txt").write_str("content")?;
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run().arg("--report-level").arg("silent"), @r#"
    success: false
    exit_code: 1
    ----- stdout -----

    ----- stderr -----
    "#);

    Ok(())
}
```

- [ ] **Step 4: Add test for `--report-level skipped-no-files`**

```rust
/// `--report-level skipped-no-files` shows failed and no-files hooks, hides passed.
#[test]
fn report_level_skipped_no_files() -> Result<()> {
    let context = TestContext::new();
    context.init_project();

    let cwd = context.work_dir();

    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: pass-hook
                name: pass-hook
                language: system
                entry: echo "ok"
                files: \.txt$
              - id: no-files-hook
                name: no-files-hook
                language: system
                entry: echo "checking"
                files: \.py$
    "#});

    cwd.child("file.txt").write_str("content")?;
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run().arg("--report-level").arg("skipped-no-files"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    no-files-hook....................................(no files to check)Skipped

    ----- stderr -----
    "#);

    Ok(())
}
```

- [ ] **Step 5: Add test for `--report-level skipped` showing excluded hooks**

```rust
/// `--report-level skipped` shows hooks excluded by --skip.
#[test]
fn report_level_skipped_shows_excluded_hooks() -> Result<()> {
    let context = TestContext::new();
    context.init_project();

    let cwd = context.work_dir();

    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: pass-hook
                name: pass-hook
                language: system
                entry: echo "ok"
                files: \.txt$
              - id: skipped-hook
                name: skipped-hook
                language: system
                entry: echo "should be skipped"
                files: \.txt$
    "#});

    cwd.child("file.txt").write_str("content")?;
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run()
        .arg("--skip").arg("skipped-hook")
        .arg("--report-level").arg("skipped"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    pass-hook...............................................................Passed
    skipped-hook........................................(excluded by skip)Skipped

    ----- stderr -----
    "#);

    Ok(())
}
```

- [ ] **Step 6: Add test for `PREK_REPORT_LEVEL` env var**

```rust
/// PREK_REPORT_LEVEL env var works as fallback for --report-level.
#[test]
fn report_level_env_var() -> Result<()> {
    let context = TestContext::new();
    context.init_project();

    let cwd = context.work_dir();

    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: pass-hook
                name: pass-hook
                language: system
                entry: echo "ok"
                files: \.txt$
              - id: fail-hook
                name: fail-hook
                language: system
                entry: exit 1
                files: \.txt$
    "#});

    cwd.child("file.txt").write_str("content")?;
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run().env("PREK_REPORT_LEVEL", "fail"), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    fail-hook...............................................................Failed
    - hook id: fail-hook
    - exit code: 1

    ----- stderr -----
    "#);

    Ok(())
}
```

- [ ] **Step 7: Add test for default behavior (no flag)**

```rust
/// Default behavior (no --report-level flag) matches current behavior.
#[test]
fn report_level_default_matches_current_behavior() -> Result<()> {
    let context = TestContext::new();
    context.init_project();

    let cwd = context.work_dir();

    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: pass-hook
                name: pass-hook
                language: system
                entry: echo "ok"
                files: \.txt$
              - id: no-files-hook
                name: no-files-hook
                language: system
                entry: echo "checking"
                files: \.py$
    "#});

    cwd.child("file.txt").write_str("content")?;
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    pass-hook...............................................................Passed
    no-files-hook....................................(no files to check)Skipped

    ----- stderr -----
    "#);

    Ok(())
}
```

- [ ] **Step 8: Run all new tests**

Run: `cargo nextest run -p prek --test skipped_hooks`
Expected: all tests pass (update snapshots with `cargo insta review` if needed)

- [ ] **Step 9: Run existing test suite to check for regressions**

Run: `cargo nextest run -p prek --test run`
Expected: all existing tests still pass

- [ ] **Step 10: Commit**

```bash
git add crates/prek/tests/skipped_hooks.rs
git commit -m "test: add integration tests for --report-level flag"
```

---

### Task 8: Final verification

- [ ] **Step 1: Run the full test suite**

Run: `cargo nextest run -p prek`
Expected: all tests pass

- [ ] **Step 2: Run clippy**

Run: `cargo clippy -p prek -- -D warnings`
Expected: no warnings

- [ ] **Step 3: Verify CLI help output**

Run: `cargo run -p prek -- run --help`
Expected: `--report-level` appears in the help output with the six level options listed

- [ ] **Step 4: Manual smoke test**

Create a temporary test project and verify:
```bash
# In a temp git repo with a .pre-commit-config.yaml:
cargo run -p prek -- run --report-level fail
cargo run -p prek -- run --report-level silent
cargo run -p prek -- run --report-level skipped --skip some-hook
```
Expected: output matches the design spec for each level
