use crate::common::{TestEnv, cmd_snapshot};

/// Test multiple hook IDs scenarios.
#[test]
fn multiple_hook_ids() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: hook1
                name: First Hook
                language: system
                entry: echo hook1
              - id: hook2
                name: Second Hook
                language: system
                entry: echo hook2
              - id: shared-name
                name: Shared Hook A
                language: system
                entry: echo shared-a
              - id: shared-name-2
                name: Shared Hook B
                language: system
                entry: echo shared-b
                alias: shared-name
    "})
        .init_git();

    // Multiple repeated hook-id (should deduplicate)
    cmd_snapshot!(context, context.run().arg("hook1").arg("hook1").arg("hook1"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    First Hook...............................................................Passed

    ----- stderr -----
    "#);

    // Hook-id that matches multiple hooks (by alias)
    cmd_snapshot!(context, context.run().arg("shared-name"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    Shared Hook A............................................................Passed
    Shared Hook B............................................................Passed

    ----- stderr -----
    "#);

    // Hook-id matches nothing
    cmd_snapshot!(context, context.run().arg("nonexistent-hook"), @r"
    success: false
    exit_code: 1
    ----- stdout -----

    ----- stderr -----
    warning: selector `nonexistent-hook` did not match any hooks
    error: No hooks found after filtering with the given selectors
    ");

    // Multiple hook_ids match nothing
    cmd_snapshot!(context, context.run().arg("nonexistent-hook").arg("nonexistent-hook").arg("nonexistent-hook-2"), @r"
    success: false
    exit_code: 1
    ----- stdout -----

    ----- stderr -----
    warning: the following selectors did not match any hooks or projects:
      - `nonexistent-hook`
      - `nonexistent-hook-2`
    error: No hooks found after filtering with the given selectors
    ");

    // Hook-id matches one hook
    cmd_snapshot!(context, context.run().arg("hook2"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    Second Hook..............................................................Passed

    ----- stderr -----
    "#);

    // Multiple hook-ids with mixed results (some exist, some don't)
    cmd_snapshot!(context, context.run().arg("hook1").arg("nonexistent").arg("hook2"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    First Hook...............................................................Passed
    Second Hook..............................................................Passed

    ----- stderr -----
    warning: selector `nonexistent` did not match any hooks
    ");

    // Multiple valid hook-ids
    cmd_snapshot!(context, context.run().arg("hook1").arg("hook2").arg("nonexistent-hook"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    First Hook...............................................................Passed
    Second Hook..............................................................Passed

    ----- stderr -----
    warning: selector `nonexistent-hook` did not match any hooks
    ");

    // Multiple hook-ids with some duplicates and aliases
    cmd_snapshot!(context, context.run().arg("hook1").arg("shared-name").arg("hook1"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    First Hook...............................................................Passed
    Shared Hook A............................................................Passed
    Shared Hook B............................................................Passed

    ----- stderr -----
    "#);
}

#[test]
fn run_group_without_stage_selects_hooks_across_stages() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: ci-files
                name: CI Files
                language: system
                entry: python3 -c "print('ci-files')"
                always_run: true
                stages: [pre-commit]
                groups: [ci]
              - id: ci-check
                name: CI Check
                language: system
                entry: python3 -c "print('ci')"
                always_run: true
                stages: [pre-push]
                groups: [ci]
              - id: commit-msg-check
                name: Commit Msg Check
                language: system
                entry: python3 -c "raise SystemExit('commit-msg hook should not run')"
                always_run: true
                stages: [commit-msg]
                groups: [ci]
              - id: prepare-commit-msg-check
                name: Prepare Commit Msg Check
                language: system
                entry: python3 -c "raise SystemExit('prepare-commit-msg hook should not run')"
                always_run: true
                stages: [prepare-commit-msg]
                groups: [ci]
              - id: local-check
                name: Local Check
                language: system
                entry: python3 -c "print('local')"
                always_run: true
                stages: [pre-commit]
    "#})
        .init_git();

    cmd_snapshot!(context, context.run().arg("--all-files").arg("--group").arg("ci"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    CI Files.................................................................Passed
    CI Check.................................................................Passed

    ----- stderr -----
    "#);
}

#[test]
fn run_ungrouped_group_selects_hooks_without_groups() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: ungrouped
                name: Ungrouped
                language: system
                entry: python3 -c "print('ungrouped')"
                always_run: true
              - id: ci
                name: CI
                language: system
                entry: python3 -c "print('ci')"
                always_run: true
                groups: [ci]
              - id: other
                name: Other
                language: system
                entry: python3 -c "print('other')"
                always_run: true
                groups: [other]

          - repo: https://notexistentatallnevergonnahappen.com/nonexistent/repo
            rev: v1.0.0
            hooks:
              - id: remote-other
                groups: [other]
    "#})
        .init_git();

    cmd_snapshot!(context,
        context
            .run()
            .arg("--all-files")
            .arg("--group")
            .arg("ci")
            .arg("--group")
            .arg("@ungrouped"),
        @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    Ungrouped................................................................Passed
    CI.......................................................................Passed

    ----- stderr -----
    "#
    );
}

#[test]
fn run_required_group_without_stage_warns_when_only_message_file_hooks_match() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: commit-msg-check
                name: Commit Msg Check
                language: system
                entry: python3 -c "print('commit-msg')"
                always_run: true
                stages: [commit-msg]
                groups: [ci]
              - id: prepare-commit-msg-check
                name: Prepare Commit Msg Check
                language: system
                entry: python3 -c "print('prepare-commit-msg')"
                always_run: true
                stages: [prepare-commit-msg]
                groups: [ci]
    "#})
        .init_git();

    cmd_snapshot!(context, context.run().arg("--all-files").arg("--require-group").arg("ci"), @r#"
    success: false
    exit_code: 1
    ----- stdout -----

    ----- stderr -----
    warning: all hooks selected by group filters require `commit-msg` or `prepare-commit-msg` stage and were not run; pass `--stage commit-msg` or `--stage prepare-commit-msg` to run them
    "#);
}

#[test]
fn run_no_group_excludes_matching_hooks_and_keeps_ungrouped_hooks() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: format
                name: Format
                language: system
                entry: python3 -c "print('format')"
                always_run: true
                groups: [format]
              - id: lint
                name: Lint
                language: system
                entry: python3 -c "print('lint')"
                always_run: true
    "#})
        .init_git();

    cmd_snapshot!(context, context.run().arg("--all-files").arg("--no-group").arg("format"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    Lint.....................................................................Passed

    ----- stderr -----
    "#);
}

#[test]
fn run_group_exclusion_wins_over_inclusion() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: fast-lint
                name: Fast Lint
                language: system
                entry: python3 -c "print('fast')"
                always_run: true
                groups: [ci]
              - id: slow-lint
                name: Slow Lint
                language: system
                entry: python3 -c "print('slow')"
                always_run: true
                groups: [ci, slow]
    "#})
        .init_git();

    cmd_snapshot!(context, context.run().arg("--all-files").arg("--group").arg("ci").arg("--no-group").arg("slow"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    Fast Lint................................................................Passed

    ----- stderr -----
    "#);
}

#[test]
fn run_required_groups_intersect_and_compose_with_other_group_filters() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: ty
                name: Ty
                language: system
                entry: python3 -c "print('ty')"
                always_run: true
                groups: [lint, fast, local]
              - id: ruff-format
                name: Ruff Format
                language: system
                entry: python3 -c "print('ruff-format')"
                always_run: true
                groups: [format, fast, local]
              - id: mypy
                name: Mypy
                language: system
                entry: python3 -c "print('mypy')"
                always_run: true
                groups: [lint, slow, ci]
              - id: black
                name: Black
                language: system
                entry: python3 -c "print('black')"
                always_run: true
                groups: [format, slow, ci]
              - id: black-fast
                name: Black Fast
                language: system
                entry: python3 -c "print('black-fast')"
                always_run: true
                groups: [format, slow, ci, fast]
    "#})
        .init_git();

    cmd_snapshot!(context,
        context
            .run()
            .arg("--all-files")
            .arg("--no-group")
            .arg("fast")
            .arg("--require-group")
            .arg("format")
            .arg("--group")
            .arg("lint")
            .arg("--no-group")
            .arg("local")
            .arg("--require-group")
            .arg("slow")
            .arg("--group")
            .arg("ci"),
        @r"
    success: true
    exit_code: 0
    ----- stdout -----
    Black....................................................................Passed

    ----- stderr -----
    "
    );

    cmd_snapshot!(context,
        context
            .run()
            .arg("--all-files")
            .arg("--group")
            .arg("ci")
            .arg("--require-group")
            .arg("slow")
            .arg("--no-group")
            .arg("local")
            .arg("--group")
            .arg("lint")
            .arg("--require-group")
            .arg("format")
            .arg("--no-group")
            .arg("fast"),
        @r"
    success: true
    exit_code: 0
    ----- stdout -----
    Black....................................................................Passed

    ----- stderr -----
    "
    );

    cmd_snapshot!(context,
        context
            .run()
            .arg("--all-files")
            .arg("--require-group")
            .arg("lint")
            .arg("--require-group")
            .arg("format"),
        @r"
    success: false
    exit_code: 1
    ----- stdout -----

    ----- stderr -----
    error: No hooks found after filtering with the given selectors
    "
    );
}

#[test]
fn run_unknown_group_selectors_warn_and_empty_selection_fails() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: lint
                name: Lint
                language: system
                entry: python3 -c "print('lint')"
                always_run: true
                groups: [ci]
    "#})
        .init_git();

    cmd_snapshot!(context, context.run().arg("--all-files").arg("--group").arg("missing"), @r#"
    success: false
    exit_code: 1
    ----- stdout -----

    ----- stderr -----
    warning: group selector `--group=missing` did not match any hooks
    error: No hooks found after filtering with the given selectors
    "#);

    cmd_snapshot!(context, context.run().arg("--all-files").arg("--require-group").arg("missing"), @r#"
    success: false
    exit_code: 1
    ----- stdout -----

    ----- stderr -----
    warning: group selector `--require-group=missing` did not match any hooks
    error: No hooks found after filtering with the given selectors
    "#);
}

#[test]
fn run_group_selectors_reject_invalid_names() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: lint
                name: Lint
                language: system
                entry: python3 -c "print('lint')"
                always_run: true
                groups: [ci]
    "#})
        .init_git();

    cmd_snapshot!(context, context.run().arg("--all-files").arg("--group").arg("ci slow"), @r#"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Invalid group selector: `--group=ci slow`
      caused by: group name cannot contain whitespace
    "#);

    cmd_snapshot!(context, context.run().arg("--all-files").arg("--no-group").arg("ci slow"), @r#"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Invalid group selector: `--no-group=ci slow`
      caused by: group name cannot contain whitespace
    "#);

    cmd_snapshot!(context, context.run().arg("--all-files").arg("--group").arg("@custom"), @r#"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Invalid group selector: `--group=@custom`
      caused by: group name uses the reserved `@` prefix
    "#);
}

#[test]
fn run_required_group_and_stage_filters_intersect() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: ci-push
                name: CI Push
                language: system
                entry: python3 -c "print('ci-push')"
                always_run: true
                stages: [pre-push]
                groups: [ci]
              - id: ci-manual
                name: CI Manual
                language: system
                entry: python3 -c "print('ci-manual')"
                always_run: true
                stages: [manual]
                groups: [ci]
              - id: other-push
                name: Other Push
                language: system
                entry: python3 -c "print('other-push')"
                always_run: true
                stages: [pre-push]
                groups: [other]
    "#})
        .init_git();

    cmd_snapshot!(context, context.run().arg("--all-files").arg("--require-group").arg("ci").arg("--stage").arg("pre-push"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    CI Push..................................................................Passed

    ----- stderr -----
    "#);
}

/// Skips hooks based on the `SKIP` environment variable.
#[test]
fn skips() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: trailing-whitespace
                name: trailing-whitespace
                language: system
                entry: python3 -c "exit(1)"
              - id: end-of-file-fixer
                name: fix end of files
                language: system
                entry: python3 -c "exit(1)"
              - id: check-json
                name: check json
                language: system
                entry: python3 -c "exit(1)"
    "#})
        .init_git();

    cmd_snapshot!(context, context.run().env("SKIP", "end-of-file-fixer"), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    trailing-whitespace......................................................Failed
    - hook id: trailing-whitespace
    - exit code: 1
    fix end of files........................................................Skipped
    check json...............................................................Failed
    - hook id: check-json
    - exit code: 1

    ----- stderr -----
    "#);

    cmd_snapshot!(context, context.run().env("SKIP", "trailing-whitespace,end-of-file-fixer"), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    trailing-whitespace.....................................................Skipped
    fix end of files........................................................Skipped
    check json...............................................................Failed
    - hook id: check-json
    - exit code: 1

    ----- stderr -----
    "#);
}

/// Run hooks with matched `stage`.
#[test]
fn stage() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: manual-stage
                name: manual-stage
                language: system
                entry: echo manual-stage
                stages: [ manual ]
              # Defaults to all stages.
              - id: default-stage
                name: default-stage
                language: system
                entry: echo default-stage
              - id: post-commit-stage
                name: post-commit-stage
                language: system
                entry: echo post-commit-stage
                stages: [ post-commit ]
    "})
        .init_git();

    // By default, run hooks with `pre-commit` stage.
    cmd_snapshot!(context, context.run(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    default-stage............................................................Passed

    ----- stderr -----
    "#);

    // Run hooks with `manual` stage.
    cmd_snapshot!(context, context.run().arg("--hook-stage").arg("manual"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    manual-stage.............................................................Passed
    default-stage............................................................Passed

    ----- stderr -----
    "#);

    // Run hooks with `post-commit` stage.
    cmd_snapshot!(context, context.run().arg("--hook-stage").arg("post-commit"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    default-stage........................................(no files to check)Skipped
    post-commit-stage....................................(no files to check)Skipped

    ----- stderr -----
    "#);
}

#[test]
fn fallback_to_manual_stage() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: manual-only
                name: manual-only
                language: system
                entry: echo manual-only
                stages: [ manual ]
              - id: another-manual
                name: another-manual
                language: system
                entry: echo another-manual
                stages: [ manual ]
              - id: default-stage
                name: default-stage
                language: system
                entry: echo default-stage
              - id: pre-push
                name: pre-push
                language: system
                entry: echo pre-push
                stages: [ pre-push ]
    "})
        .init_git();

    // With pre-commit hooks present, default `prek run` stays on pre-commit.
    cmd_snapshot!(context, context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    default-stage............................................................Passed

    ----- stderr -----
    ");

    // Explicit `--hook-stage pre-commit` keeps execution scoped to that stage.
    cmd_snapshot!(context, context.run().arg("--hook-stage").arg("pre-commit").arg("default-stage").arg("manual-only"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    default-stage............................................................Passed

    ----- stderr -----
    ");

    // Selecting manual + pre-commit hooks still runs only the pre-commit ones.
    cmd_snapshot!(context, context.run().arg("manual-only").arg("default-stage"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    default-stage............................................................Passed

    ----- stderr -----
    ");

    // A skipped pre-commit hook should not prevent a runnable manual hook from falling back.
    cmd_snapshot!(context, context.run().arg("--skip").arg("default-stage").arg("manual-only").arg("default-stage"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    manual-only..............................................................Passed
    default-stage...........................................................Skipped

    ----- stderr -----
    "#);

    // Selecting only manual hooks should still succeed via fallback.
    cmd_snapshot!(context, context.run().arg("manual-only").arg("another-manual"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    manual-only..............................................................Passed
    another-manual...........................................................Passed

    ----- stderr -----
    ");

    // Mixing `pre-push` and manual selectors still runs the manual hook via fallback.
    cmd_snapshot!(context, context.run().arg("pre-push").arg("manual-only"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    manual-only..............................................................Passed

    ----- stderr -----
    ");
}
