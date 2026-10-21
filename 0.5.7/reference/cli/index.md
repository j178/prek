# CLI Reference

Running `prek` without a subcommand is equivalent to running `prek run`.

## Exit status

| Code | Meaning |
| --- | --- |
| `0` | The command succeeded. |
| `1` | A hook, validation, or other expected user-level check failed. |
| `2` | Command-line input, configuration, or an operational error prevented the command from completing. |
| `130` | The command was interrupted. |

`prek exec` propagates the exit code of the external command it runs.

## prek

A fast Git hook manager written in Rust, designed as a drop-in alternative to pre-commit, reimagined.

### Usage

```text
prek [OPTIONS] [HOOK|PROJECT]... [COMMAND]
```

### Commands

**[`prek init`](<#prek-init>)**

Create a prek configuration and install Git hook shims

**[`prek install`](<#prek-install>)**

Install prek Git hook shims

**[`prek prepare-hooks`](<#prek-prepare-hooks>)**

Prepare environments for configured hooks

**[`prek run`](<#prek-run>)**

Run configured hooks

**[`prek exec`](<#prek-exec>)**

Run a command in the environment prepared for a configured hook

**[`prek list`](<#prek-list>)**

List configured hooks

**[`prek uninstall`](<#prek-uninstall>)**

Uninstall prek Git hook shims

**[`prek validate-config`](<#prek-validate-config>)**

Validate prek configuration files

**[`prek validate-manifest`](<#prek-validate-manifest>)**

Validate pre-commit hook manifests (`.pre-commit-hooks.yaml`)

**[`prek update`](<#prek-update>)**

Update configured repositories

**[`prek cache`](<#prek-cache>)**

Manage the prek cache

**[`prek try-repo`](<#prek-try-repo>)**

Try hooks from a repository

**[`prek util`](<#prek-util>)**

Run utility commands

**[`prek self`](<#prek-self>)**

Manage the prek installation

## prek init

Create a prek configuration and install Git hook shims

### Usage

```text
prek init [OPTIONS] [PATH]
```

### Arguments

**[`PATH`](<#prek-init--path>)**

Existing directory to initialize.

Defaults to the current Git worktree root. Relative paths are resolved from the current directory after applying `--cd`, and the resolved path must be inside the current Git worktree.

### Options

**[`--cd`](<#prek-init--cd>), `-C` *dir***

Change to directory before running

**[`--color`](<#prek-init--color>) *color***

Whether to use color in output

May also be set with the `PREK_COLOR` environment variable.

\[default: auto\]

Possible values:

- `auto`: Enables colored output only when the output is going to a terminal or TTY with support
- `always`: Enables colored output regardless of the detected environment
- `never`: Disables colored output

**[`--config`](<#prek-init--config>), `-c` *config***

Path to alternate config file

**[`--format`](<#prek-init--format>) *format***

Select the configuration format to create

\[default: toml\]

Possible values:

- `yaml`
- `toml`

**[`--help`](<#prek-init--help>), `-h`**

Display the concise help for this command

**[`--log-file`](<#prek-init--log-file>) *log-file***

Write trace logs to the specified file. If not specified, trace logs will be written to `$PREK_HOME/prek.log`

**[`--no-install`](<#prek-init--no-install>)**

Do not install Git hook shims

**[`--no-progress`](<#prek-init--no-progress>)**

Hide all progress outputs.

For example, spinners or progress bars.

**[`--quiet`](<#prek-init--quiet>), `-q`**

Use quiet output.

Repeating this option, e.g., `-qq`, will enable a silent mode in which prek will write no output to stdout.

May also be set with the `PREK_QUIET` environment variable.

**[`--refresh`](<#prek-init--refresh>)**

Refresh all cached data

**[`--verbose`](<#prek-init--verbose>), `-v`**

Use verbose output

**[`--version`](<#prek-init--version>), `-V`**

Display the prek version

## prek install

Install prek Git hook shims.

The effective hooks directory defaults to `.git/hooks/`, but repo-local or worktree-local `core.hooksPath` is honored when set.

The Git shims installed by this command are determined by `--hook-type` or `default_install_hook_types` in the config file, falling back to `pre-commit` when neither is set.

A hook's `stages` field does not affect which Git shims this command installs.

### Usage

```text
prek install [OPTIONS] [HOOK|PROJECT]...
```

### Arguments

**[`HOOK|PROJECT`](<#prek-install--includes>)**

Include the specified hooks or projects.

Supports flexible selector syntax:

- `hook-id`: Run all hooks with the specified ID across all projects
- `project-path/`: Run all hooks from the specified project
- `project-path:hook-id`: Run only the specified hook from the specified project

Can be specified multiple times to select multiple hooks/projects.

### Options

**[`--allow-missing-config`](<#prek-install--allow-missing-config>)**

Allow a missing configuration file

**[`--cd`](<#prek-install--cd>), `-C` *dir***

Change to directory before running

**[`--color`](<#prek-install--color>) *color***

Whether to use color in output

May also be set with the `PREK_COLOR` environment variable.

\[default: auto\]

Possible values:

- `auto`: Enables colored output only when the output is going to a terminal or TTY with support
- `always`: Enables colored output regardless of the detected environment
- `never`: Disables colored output

**[`--config`](<#prek-install--config>), `-c` *config***

Path to alternate config file

**[`--force`](<#prek-install--force>), `--overwrite`, `-f`**

Force installation and overwrite existing Git shims.

If `core.hooksPath` is configured outside this repository, install the shims into this repository's default hooks directory.

**[`--git-dir`](<#prek-install--git-dir>) *git-dir***

Install Git shims into the `hooks` subdirectory of the given git directory (`<GIT_DIR>/hooks/`).

When this flag is used, `prek install` bypasses the safety check that normally refuses to install shims while `core.hooksPath` is configured outside the repo. It only writes shims to `<GIT_DIR>/hooks`; Git will keep using `core.hooksPath` until that config changes.

**[`--help`](<#prek-install--help>), `-h`**

Display the concise help for this command

**[`--hook-type`](<#prek-install--hook-type>), `-t` *hook-type***

Which Git shim(s) to install.

Specifies which Git hook type(s) you want to install shims for. Can be specified multiple times to install shims for multiple hook types.

If not specified, uses `default_install_hook_types` from the config file, or defaults to `pre-commit` if that is also not set.

Note: This is different from a hook's `stages` parameter in the config file, which declares which stages a hook *can* run in.

Possible values:

- `commit-msg`
- `post-checkout`
- `post-commit`
- `post-merge`
- `post-rewrite`
- `pre-commit`
- `pre-merge-commit`
- `pre-push`
- `pre-rebase`
- `prepare-commit-msg`

**[`--log-file`](<#prek-install--log-file>) *log-file***

Write trace logs to the specified file. If not specified, trace logs will be written to `$PREK_HOME/prek.log`

**[`--no-progress`](<#prek-install--no-progress>)**

Hide all progress outputs.

For example, spinners or progress bars.

**[`--prepare-hooks`](<#prek-install--prepare-hooks>), `--install-hooks`**

Also prepare environments for all hooks used in the config file

**[`--quiet`](<#prek-install--quiet>), `-q`**

Use quiet output.

Repeating this option, e.g., `-qq`, will enable a silent mode in which prek will write no output to stdout.

May also be set with the `PREK_QUIET` environment variable.

**[`--refresh`](<#prek-install--refresh>)**

Refresh all cached data

**[`--skip`](<#prek-install--skip>) *hook|project***

Skip the specified hooks or projects.

Supports flexible selector syntax:

- `hook-id`: Skip all hooks with the specified ID across all projects
- `project-path/`: Skip all hooks from the specified project
- `project-path:hook-id`: Skip only the specified hook from the specified project

Can be specified multiple times. Also accepts `PREK_SKIP` or `SKIP` environment variables (comma-delimited).

**[`--verbose`](<#prek-install--verbose>), `-v`**

Use verbose output

**[`--version`](<#prek-install--version>), `-V`**

Display the prek version

## prek prepare-hooks

Prepare environments for configured hooks.

This command does not install Git shims. To install the Git shims along with the hook environments in one command, use `prek install --prepare-hooks`.

### Usage

```text
prek prepare-hooks [OPTIONS] [HOOK|PROJECT]...
```

### Arguments

**[`HOOK|PROJECT`](<#prek-prepare-hooks--includes>)**

Include the specified hooks or projects.

Supports flexible selector syntax:

- `hook-id`: Run all hooks with the specified ID across all projects
- `project-path/`: Run all hooks from the specified project
- `project-path:hook-id`: Run only the specified hook from the specified project

Can be specified multiple times to select multiple hooks/projects.

### Options

**[`--cd`](<#prek-prepare-hooks--cd>), `-C` *dir***

Change to directory before running

**[`--color`](<#prek-prepare-hooks--color>) *color***

Whether to use color in output

May also be set with the `PREK_COLOR` environment variable.

\[default: auto\]

Possible values:

- `auto`: Enables colored output only when the output is going to a terminal or TTY with support
- `always`: Enables colored output regardless of the detected environment
- `never`: Disables colored output

**[`--config`](<#prek-prepare-hooks--config>), `-c` *config***

Path to alternate config file

**[`--help`](<#prek-prepare-hooks--help>), `-h`**

Display the concise help for this command

**[`--log-file`](<#prek-prepare-hooks--log-file>) *log-file***

Write trace logs to the specified file. If not specified, trace logs will be written to `$PREK_HOME/prek.log`

**[`--no-progress`](<#prek-prepare-hooks--no-progress>)**

Hide all progress outputs.

For example, spinners or progress bars.

**[`--quiet`](<#prek-prepare-hooks--quiet>), `-q`**

Use quiet output.

Repeating this option, e.g., `-qq`, will enable a silent mode in which prek will write no output to stdout.

May also be set with the `PREK_QUIET` environment variable.

**[`--refresh`](<#prek-prepare-hooks--refresh>)**

Refresh all cached data

**[`--skip`](<#prek-prepare-hooks--skip>) *hook|project***

Skip the specified hooks or projects.

Supports flexible selector syntax:

- `hook-id`: Skip all hooks with the specified ID across all projects
- `project-path/`: Skip all hooks from the specified project
- `project-path:hook-id`: Skip only the specified hook from the specified project

Can be specified multiple times. Also accepts `PREK_SKIP` or `SKIP` environment variables (comma-delimited).

**[`--verbose`](<#prek-prepare-hooks--verbose>), `-v`**

Use verbose output

**[`--version`](<#prek-prepare-hooks--version>), `-V`**

Display the prek version

## prek run

Run configured hooks

### Usage

```text
prek run [OPTIONS] [HOOK|PROJECT]...
```

### Arguments

**[`HOOK|PROJECT`](<#prek-run--includes>)**

Include the specified hooks or projects.

Supports flexible selector syntax:

- `hook-id`: Run all hooks with the specified ID across all projects
- `project-path/`: Run all hooks from the specified project
- `project-path:hook-id`: Run only the specified hook from the specified project

Can be specified multiple times to select multiple hooks/projects.

### Options

**[`--all-files`](<#prek-run--all-files>), `-a`**

Run hooks on all tracked files in the repository

**[`--cd`](<#prek-run--cd>), `-C` *dir***

Change to directory before running

**[`--color`](<#prek-run--color>) *color***

Whether to use color in output

May also be set with the `PREK_COLOR` environment variable.

\[default: auto\]

Possible values:

- `auto`: Enables colored output only when the output is going to a terminal or TTY with support
- `always`: Enables colored output regardless of the detected environment
- `never`: Disables colored output

**[`--config`](<#prek-run--config>), `-c` *config***

Path to alternate config file

**[`--directory`](<#prek-run--directory>), `-d` *dir***

Run hooks on tracked files under the specified directory.

Paths are resolved relative to the current working directory after applying `--cd`. This option can be repeated and combined with `--files` and `--glob`.

**[`--dry-run`](<#prek-run--dry-run>)**

Do not run the hooks, but print the hooks that would have been run

**[`--fail-fast`](<#prek-run--fail-fast>)**

Stop running hooks after the first failure

**[`--files`](<#prek-run--files>) *files***

Run hooks on the specified file paths.

Paths are resolved relative to the current working directory after applying `--cd`. They may be tracked or untracked. This option accepts multiple paths and can be combined with `--glob` and `--directory`.

**[`--from-ref`](<#prek-run--from-ref>), `--source`, `-s` *from-ref***

The original ref in a `<from_ref>...<to_ref>` diff expression. Files changed in this diff will be run through the hooks

**[`--glob`](<#prek-run--glob>) *pattern***

Run hooks on tracked files matching the specified glob pattern.

Patterns are matched against paths relative to the current working directory after applying `--cd`. Quote patterns to prevent shell expansion. This option can be repeated and combined with `--files` and `--directory`.

**[`--group`](<#prek-run--group>) *group***

Run hooks belonging to the specified group.

Can be specified multiple times; a hook may match any specified group. When combined with `--require-group`, both filters must match. `@ungrouped` matches hooks without groups.

**[`--help`](<#prek-run--help>), `-h`**

Display the concise help for this command

**[`--hide-status`](<#prek-run--hide-status>) *status***

Hide hook reports with the specified final status.

Can be specified multiple times or as a comma-separated list. This does not change hook execution or exit codes. Overrides `hide_status` in project and user configuration.

Possible values:

- `passed`
- `failed`
- `skipped`

**[`--last-commit`](<#prek-run--last-commit>)**

Run hooks against the last commit. Equivalent to `--from-ref HEAD~1 --to-ref HEAD`

**[`--log-file`](<#prek-run--log-file>) *log-file***

Write trace logs to the specified file. If not specified, trace logs will be written to `$PREK_HOME/prek.log`

**[`--no-group`](<#prek-run--no-group>) *group***

Do not run hooks belonging to the specified group.

Can be specified multiple times. Exclusion wins over inclusion. The special selector `@ungrouped` is also supported.

**[`--no-hide-status`](<#prek-run--no-hide-status>)**

Show all hook reports, overriding `hide_status` in configuration

**[`--no-progress`](<#prek-run--no-progress>)**

Hide all progress outputs.

For example, spinners or progress bars.

**[`--quiet`](<#prek-run--quiet>), `-q`**

Use quiet output.

Repeating this option, e.g., `-qq`, will enable a silent mode in which prek will write no output to stdout.

May also be set with the `PREK_QUIET` environment variable.

**[`--refresh`](<#prek-run--refresh>)**

Refresh all cached data

**[`--require-group`](<#prek-run--require-group>) *group***

Run hooks belonging to every specified group.

Can be specified multiple times; a hook must match every specified group. When combined with `--group`, it must also match at least one `--group`. `--no-group` excludes matching hooks regardless of argument order.

For example, `--require-group fast --group format --group lint-only` selects hooks in `fast` and either `format` or `lint-only`. The special selector `@ungrouped` is also supported.

**[`--show-diff-on-failure`](<#prek-run--show-diff-on-failure>)**

When hooks fail, run `git diff` directly afterward

**[`--skip`](<#prek-run--skip>) *hook|project***

Skip the specified hooks or projects.

Supports flexible selector syntax:

- `hook-id`: Skip all hooks with the specified ID across all projects
- `project-path/`: Skip all hooks from the specified project
- `project-path:hook-id`: Skip only the specified hook from the specified project

Can be specified multiple times. Also accepts `PREK_SKIP` or `SKIP` environment variables (comma-delimited).

**[`--stage`](<#prek-run--stage>), `--hook-stage` *stage***

The stage during which the hook is fired.

When specified, only hooks configured for that stage (for example `manual`, `pre-commit`, or `pre-push`) will run. When not specified and no group filter is active, this command starts with hooks eligible for `pre-commit`. If no hook is selected and the command named hook IDs, those same IDs are matched again against hooks configured for `manual`. With `--group`, `--require-group`, or `--no-group`, omitting the stage lets hooks from any configured stage match, using the default file input mode; hooks that only run at `commit-msg` or `prepare-commit-msg` are ignored.

Possible values:

- `manual`
- `commit-msg`
- `post-checkout`
- `post-commit`
- `post-merge`
- `post-rewrite`
- `pre-commit`
- `pre-merge-commit`
- `pre-push`
- `pre-rebase`
- `prepare-commit-msg`

**[`--to-ref`](<#prek-run--to-ref>), `--origin`, `-o` *to-ref***

The destination ref in a `from_ref...to_ref` diff expression. Defaults to `HEAD` if `from_ref` is specified

**[`--verbose`](<#prek-run--verbose>), `-v`**

Use verbose output

**[`--version`](<#prek-run--version>), `-V`**

Display the prek version

## prek exec

Run a command in the environment prepared for a configured hook.

The selector must resolve to exactly one hook. Its environment is prepared first if necessary, including its toolchain, dependencies, and environment variables.

Everything after `--` replaces the hook's configured `entry` and `args`. This command does not select files, schedule other hooks, or stash changes. The child process runs in the current working directory after `--cd`, inherits standard input, output, and error, and returns its exit status.

The `docker`, `docker_image`, `fail`, `julia`, and `pygrep` languages are unsupported. Builtin and meta hooks are also unsupported.

### Usage

```text
prek exec [OPTIONS] <HOOK> -- <COMMAND>...
```

### Arguments

**[`HOOK`](<#prek-exec--selector>)**

Hook whose execution environment should be used.

Supports `hook-id` and `project-path:hook-id` selectors and must resolve to exactly one configured hook.

**[`COMMAND`](<#prek-exec--command>)**

Command and arguments to execute

### Options

**[`--cd`](<#prek-exec--cd>), `-C` *dir***

Change to directory before running

**[`--color`](<#prek-exec--color>) *color***

Whether to use color in output

May also be set with the `PREK_COLOR` environment variable.

\[default: auto\]

Possible values:

- `auto`: Enables colored output only when the output is going to a terminal or TTY with support
- `always`: Enables colored output regardless of the detected environment
- `never`: Disables colored output

**[`--config`](<#prek-exec--config>), `-c` *config***

Path to alternate config file

**[`--help`](<#prek-exec--help>), `-h`**

Display the concise help for this command

**[`--log-file`](<#prek-exec--log-file>) *log-file***

Write trace logs to the specified file. If not specified, trace logs will be written to `$PREK_HOME/prek.log`

**[`--no-progress`](<#prek-exec--no-progress>)**

Hide all progress outputs.

For example, spinners or progress bars.

**[`--quiet`](<#prek-exec--quiet>), `-q`**

Use quiet output.

Repeating this option, e.g., `-qq`, will enable a silent mode in which prek will write no output to stdout.

May also be set with the `PREK_QUIET` environment variable.

**[`--refresh`](<#prek-exec--refresh>)**

Refresh all cached data

**[`--verbose`](<#prek-exec--verbose>), `-v`**

Use verbose output

**[`--version`](<#prek-exec--version>), `-V`**

Display the prek version

## prek list

List configured hooks

### Usage

```text
prek list [OPTIONS] [HOOK|PROJECT]...
```

### Arguments

**[`HOOK|PROJECT`](<#prek-list--includes>)**

Include the specified hooks or projects.

Supports flexible selector syntax:

- `hook-id`: Run all hooks with the specified ID across all projects
- `project-path/`: Run all hooks from the specified project
- `project-path:hook-id`: Run only the specified hook from the specified project

Can be specified multiple times to select multiple hooks/projects.

### Options

**[`--cd`](<#prek-list--cd>), `-C` *dir***

Change to directory before running

**[`--color`](<#prek-list--color>) *color***

Whether to use color in output

May also be set with the `PREK_COLOR` environment variable.

\[default: auto\]

Possible values:

- `auto`: Enables colored output only when the output is going to a terminal or TTY with support
- `always`: Enables colored output regardless of the detected environment
- `never`: Disables colored output

**[`--config`](<#prek-list--config>), `-c` *config***

Path to alternate config file

**[`--group`](<#prek-list--group>) *group***

Show hooks belonging to the specified group.

Can be specified multiple times. `@ungrouped` matches hooks without groups.

**[`--help`](<#prek-list--help>), `-h`**

Display the concise help for this command

**[`--hook-stage`](<#prek-list--hook-stage>) *hook-stage***

Show only hooks that has the specified stage

Possible values:

- `manual`
- `commit-msg`
- `post-checkout`
- `post-commit`
- `post-merge`
- `post-rewrite`
- `pre-commit`
- `pre-merge-commit`
- `pre-push`
- `pre-rebase`
- `prepare-commit-msg`

**[`--language`](<#prek-list--language>) *language***

Show only hooks that are implemented in the specified language

Possible values:

- `bun`
- `conda`
- `coursier`
- `dart`
- `deno`
- `docker`
- `docker-image`
- `dotnet`
- `fail`
- `golang`
- `haskell`
- `julia`
- `lua`
- `mise`
- `node`
- `perl`
- `php`
- `pygrep`
- `python`
- `r`
- `ruby`
- `rust`
- `script`
- `swift`
- `system`

**[`--log-file`](<#prek-list--log-file>) *log-file***

Write trace logs to the specified file. If not specified, trace logs will be written to `$PREK_HOME/prek.log`

**[`--no-group`](<#prek-list--no-group>) *group***

Do not show hooks belonging to the specified group.

Can be specified multiple times. Exclusion wins over inclusion. The special selector `@ungrouped` is also supported.

**[`--no-progress`](<#prek-list--no-progress>)**

Hide all progress outputs.

For example, spinners or progress bars.

**[`--output-format`](<#prek-list--output-format>) *output-format***

The output format

\[default: text\]

Possible values:

- `text`
- `json`

**[`--quiet`](<#prek-list--quiet>), `-q`**

Use quiet output.

Repeating this option, e.g., `-qq`, will enable a silent mode in which prek will write no output to stdout.

May also be set with the `PREK_QUIET` environment variable.

**[`--refresh`](<#prek-list--refresh>)**

Refresh all cached data

**[`--require-group`](<#prek-list--require-group>) *group***

Show hooks belonging to every specified group.

Can be specified multiple times. Composes with `--group` and `--no-group`. The special selector `@ungrouped` is also supported.

**[`--skip`](<#prek-list--skip>) *hook|project***

Skip the specified hooks or projects.

Supports flexible selector syntax:

- `hook-id`: Skip all hooks with the specified ID across all projects
- `project-path/`: Skip all hooks from the specified project
- `project-path:hook-id`: Skip only the specified hook from the specified project

Can be specified multiple times. Also accepts `PREK_SKIP` or `SKIP` environment variables (comma-delimited).

**[`--verbose`](<#prek-list--verbose>), `-v`**

Use verbose output

**[`--version`](<#prek-list--version>), `-V`**

Display the prek version

## prek uninstall

Uninstall prek Git hook shims

### Usage

```text
prek uninstall [OPTIONS]
```

### Options

**[`--all`](<#prek-uninstall--all>)**

Uninstall all prek-managed Git shims.

Scans the hooks directory and removes every hook managed by prek, regardless of hook type.

**[`--cd`](<#prek-uninstall--cd>), `-C` *dir***

Change to directory before running

**[`--color`](<#prek-uninstall--color>) *color***

Whether to use color in output

May also be set with the `PREK_COLOR` environment variable.

\[default: auto\]

Possible values:

- `auto`: Enables colored output only when the output is going to a terminal or TTY with support
- `always`: Enables colored output regardless of the detected environment
- `never`: Disables colored output

**[`--config`](<#prek-uninstall--config>), `-c` *config***

Path to alternate config file

**[`--git-dir`](<#prek-uninstall--git-dir>) *git-dir***

Uninstall Git shims from the `hooks` subdirectory of the given git directory (`<GIT_DIR>/hooks/`).

When this flag is used, `prek uninstall` bypasses the safety check that normally refuses to modify shims while `core.hooksPath` is configured outside the repo. It only removes shims from `<GIT_DIR>/hooks`; Git may still use the configured `core.hooksPath` until that config changes.

**[`--help`](<#prek-uninstall--help>), `-h`**

Display the concise help for this command

**[`--hook-type`](<#prek-uninstall--hook-type>), `-t` *hook-type***

Which Git shim(s) to uninstall.

Specifies which Git hook type(s) you want to uninstall shims for. Can be specified multiple times to uninstall shims for multiple hook types.

If not specified, uses `default_install_hook_types` from the config file, or defaults to `pre-commit` if that is also not set. Use `--all` to remove all prek-managed hooks.

Possible values:

- `commit-msg`
- `post-checkout`
- `post-commit`
- `post-merge`
- `post-rewrite`
- `pre-commit`
- `pre-merge-commit`
- `pre-push`
- `pre-rebase`
- `prepare-commit-msg`

**[`--log-file`](<#prek-uninstall--log-file>) *log-file***

Write trace logs to the specified file. If not specified, trace logs will be written to `$PREK_HOME/prek.log`

**[`--no-progress`](<#prek-uninstall--no-progress>)**

Hide all progress outputs.

For example, spinners or progress bars.

**[`--quiet`](<#prek-uninstall--quiet>), `-q`**

Use quiet output.

Repeating this option, e.g., `-qq`, will enable a silent mode in which prek will write no output to stdout.

May also be set with the `PREK_QUIET` environment variable.

**[`--refresh`](<#prek-uninstall--refresh>)**

Refresh all cached data

**[`--verbose`](<#prek-uninstall--verbose>), `-v`**

Use verbose output

**[`--version`](<#prek-uninstall--version>), `-V`**

Display the prek version

## prek validate-config

Validate prek configuration files

### Usage

```text
prek validate-config [OPTIONS] [CONFIG]...
```

### Arguments

**[`CONFIG`](<#prek-validate-config--configs>)**

The path to the configuration file

### Options

**[`--cd`](<#prek-validate-config--cd>), `-C` *dir***

Change to directory before running

**[`--color`](<#prek-validate-config--color>) *color***

Whether to use color in output

May also be set with the `PREK_COLOR` environment variable.

\[default: auto\]

Possible values:

- `auto`: Enables colored output only when the output is going to a terminal or TTY with support
- `always`: Enables colored output regardless of the detected environment
- `never`: Disables colored output

**[`--config`](<#prek-validate-config--config>), `-c` *config***

Path to alternate config file

**[`--help`](<#prek-validate-config--help>), `-h`**

Display the concise help for this command

**[`--log-file`](<#prek-validate-config--log-file>) *log-file***

Write trace logs to the specified file. If not specified, trace logs will be written to `$PREK_HOME/prek.log`

**[`--no-progress`](<#prek-validate-config--no-progress>)**

Hide all progress outputs.

For example, spinners or progress bars.

**[`--quiet`](<#prek-validate-config--quiet>), `-q`**

Use quiet output.

Repeating this option, e.g., `-qq`, will enable a silent mode in which prek will write no output to stdout.

May also be set with the `PREK_QUIET` environment variable.

**[`--refresh`](<#prek-validate-config--refresh>)**

Refresh all cached data

**[`--verbose`](<#prek-validate-config--verbose>), `-v`**

Use verbose output

**[`--version`](<#prek-validate-config--version>), `-V`**

Display the prek version

## prek validate-manifest

Validate pre-commit hook manifests (`.pre-commit-hooks.yaml`)

### Usage

```text
prek validate-manifest [OPTIONS] [MANIFEST]...
```

### Arguments

**[`MANIFEST`](<#prek-validate-manifest--manifests>)**

The path to the manifest file

### Options

**[`--cd`](<#prek-validate-manifest--cd>), `-C` *dir***

Change to directory before running

**[`--color`](<#prek-validate-manifest--color>) *color***

Whether to use color in output

May also be set with the `PREK_COLOR` environment variable.

\[default: auto\]

Possible values:

- `auto`: Enables colored output only when the output is going to a terminal or TTY with support
- `always`: Enables colored output regardless of the detected environment
- `never`: Disables colored output

**[`--config`](<#prek-validate-manifest--config>), `-c` *config***

Path to alternate config file

**[`--help`](<#prek-validate-manifest--help>), `-h`**

Display the concise help for this command

**[`--log-file`](<#prek-validate-manifest--log-file>) *log-file***

Write trace logs to the specified file. If not specified, trace logs will be written to `$PREK_HOME/prek.log`

**[`--no-progress`](<#prek-validate-manifest--no-progress>)**

Hide all progress outputs.

For example, spinners or progress bars.

**[`--quiet`](<#prek-validate-manifest--quiet>), `-q`**

Use quiet output.

Repeating this option, e.g., `-qq`, will enable a silent mode in which prek will write no output to stdout.

May also be set with the `PREK_QUIET` environment variable.

**[`--refresh`](<#prek-validate-manifest--refresh>)**

Refresh all cached data

**[`--verbose`](<#prek-validate-manifest--verbose>), `-v`**

Use verbose output

**[`--version`](<#prek-validate-manifest--version>), `-V`**

Display the prek version

## prek update

Update configured repositories

### Usage

```text
prek update [OPTIONS]
```

### Options

**[`--bleeding-edge`](<#prek-update--bleeding-edge>)**

Update to the bleeding edge of the default branch instead of the latest tagged version

**[`--cd`](<#prek-update--cd>), `-C` *dir***

Change to directory before running

**[`--check`](<#prek-update--check>)**

Alias of `--dry-run --exit-code`

**[`--color`](<#prek-update--color>) *color***

Whether to use color in output

May also be set with the `PREK_COLOR` environment variable.

\[default: auto\]

Possible values:

- `auto`: Enables colored output only when the output is going to a terminal or TTY with support
- `always`: Enables colored output regardless of the detected environment
- `never`: Disables colored output

**[`--config`](<#prek-update--config>), `-c` *config***

Path to alternate config file

**[`--cooldown-days`](<#prek-update--cooldown-days>) *days***

Minimum release age (in days) required for a version to be eligible.

The age is computed from the tag creation timestamp for annotated tags, or from the tagged commit timestamp for lightweight tags. If the current `rev` is newer than the latest cooldown-eligible tag, `prek update` keeps the current `rev` instead of downgrading it. Defaults to `update.cooldown_days` in the project or global config, or `0` when unset. Valid values are `0` through `255`; `0` disables this check.

**[`--dry-run`](<#prek-update--dry-run>)**

Do not write changes to the config file, only display what would be changed

**[`--exclude-repo`](<#prek-update--exclude-repo>) *repo***

Do not update this repository. This option may be specified multiple times

**[`--exclude-tag`](<#prek-update--exclude-tag>) *pattern***

Ignore tags matching this glob pattern. This option may be specified multiple times. Defaults to `update.exclude_tags` in the project or global config when unset.

For example, use `--exclude-tag nightly` to skip a moving tag, or `--exclude-tag '*-{alpha,beta,rc}*'` to skip common prerelease tags.

**[`--exit-code`](<#prek-update--exit-code>)**

Exit with status 1 if updates are available

**[`--freeze`](<#prek-update--freeze>)**

Store "frozen" hashes in `rev` instead of tag names. Defaults to `update.freeze` in the project or global config, or `false` when unset

**[`--help`](<#prek-update--help>), `-h`**

Display the concise help for this command

**[`--include-tag`](<#prek-update--include-tag>) *pattern***

Only consider tags matching this glob pattern. This option may be specified multiple times. Defaults to `update.include_tags` in the project or global config when unset.

For example, use `--include-tag 'v*'` to only consider version tags and ignore tags such as `nightly`.

**[`--jobs`](<#prek-update--jobs>), `-j` *jobs***

Number of threads to use

\[default: 0\]

**[`--log-file`](<#prek-update--log-file>) *log-file***

Write trace logs to the specified file. If not specified, trace logs will be written to `$PREK_HOME/prek.log`

**[`--no-progress`](<#prek-update--no-progress>)**

Hide all progress outputs.

For example, spinners or progress bars.

**[`--quiet`](<#prek-update--quiet>), `-q`**

Use quiet output.

Repeating this option, e.g., `-qq`, will enable a silent mode in which prek will write no output to stdout.

May also be set with the `PREK_QUIET` environment variable.

**[`--refresh`](<#prek-update--refresh>)**

Refresh all cached data

**[`--repo`](<#prek-update--repo>) *repo***

Only update this repository. This option may be specified multiple times

**[`--repo-exclude-tag`](<#prek-update--repo-exclude-tag>) *repo=pattern***

Ignore tags matching this glob pattern for a repository (`<repo>=<pattern>`). This option may be specified multiple times. Adds to the effective `update` exclude filters for the named repository.

Repo-specific exclude filters are added to global `--exclude-tag` filters; matching either filter excludes the tag for that repository.

For example, use `--repo-exclude-tag https://github.com/example/repo=nightly` or `--repo-exclude-tag https://github.com/example/repo=*-rc*` to skip nightly or prerelease tags for one repository.

**[`--repo-include-tag`](<#prek-update--repo-include-tag>) *repo=pattern***

Only consider tags matching this glob pattern for a repository (`<repo>=<pattern>`). This option may be specified multiple times. Overrides the effective include filters for the named repository.

When set for a repository, this overrides any global `--include-tag` filters for that repository.

For example, use `--repo-include-tag https://github.com/example/repo=v*` to only consider version tags for one repository.

**[`--verbose`](<#prek-update--verbose>), `-v`**

Use verbose output

**[`--version`](<#prek-update--version>), `-V`**

Display the prek version

## prek cache

Manage the prek cache

### Usage

```text
prek cache [OPTIONS] <COMMAND>
```

### Commands

**[`prek cache dir`](<#prek-cache-dir>)**

Show the location of the prek cache

**[`prek cache gc`](<#prek-cache-gc>)**

Remove unused cached repositories, hook environments, and other data

**[`prek cache clean`](<#prek-cache-clean>)**

Remove all prek cached data

**[`prek cache size`](<#prek-cache-size>)**

Show the size of the prek cache

### prek cache dir

Show the location of the prek cache

### Usage

```text
prek cache dir [OPTIONS]
```

### Options

**[`--cd`](<#prek-cache-dir--cd>), `-C` *dir***

Change to directory before running

**[`--color`](<#prek-cache-dir--color>) *color***

Whether to use color in output

May also be set with the `PREK_COLOR` environment variable.

\[default: auto\]

Possible values:

- `auto`: Enables colored output only when the output is going to a terminal or TTY with support
- `always`: Enables colored output regardless of the detected environment
- `never`: Disables colored output

**[`--config`](<#prek-cache-dir--config>), `-c` *config***

Path to alternate config file

**[`--help`](<#prek-cache-dir--help>), `-h`**

Display the concise help for this command

**[`--log-file`](<#prek-cache-dir--log-file>) *log-file***

Write trace logs to the specified file. If not specified, trace logs will be written to `$PREK_HOME/prek.log`

**[`--no-progress`](<#prek-cache-dir--no-progress>)**

Hide all progress outputs.

For example, spinners or progress bars.

**[`--quiet`](<#prek-cache-dir--quiet>), `-q`**

Use quiet output.

Repeating this option, e.g., `-qq`, will enable a silent mode in which prek will write no output to stdout.

May also be set with the `PREK_QUIET` environment variable.

**[`--refresh`](<#prek-cache-dir--refresh>)**

Refresh all cached data

**[`--verbose`](<#prek-cache-dir--verbose>), `-v`**

Use verbose output

**[`--version`](<#prek-cache-dir--version>), `-V`**

Display the prek version

### prek cache gc

Remove unused cached repositories, hook environments, and other data

### Usage

```text
prek cache gc [OPTIONS]
```

### Options

**[`--cd`](<#prek-cache-gc--cd>), `-C` *dir***

Change to directory before running

**[`--color`](<#prek-cache-gc--color>) *color***

Whether to use color in output

May also be set with the `PREK_COLOR` environment variable.

\[default: auto\]

Possible values:

- `auto`: Enables colored output only when the output is going to a terminal or TTY with support
- `always`: Enables colored output regardless of the detected environment
- `never`: Disables colored output

**[`--config`](<#prek-cache-gc--config>), `-c` *config***

Path to alternate config file

**[`--dry-run`](<#prek-cache-gc--dry-run>)**

Print what would be removed, but do not delete anything

**[`--help`](<#prek-cache-gc--help>), `-h`**

Display the concise help for this command

**[`--log-file`](<#prek-cache-gc--log-file>) *log-file***

Write trace logs to the specified file. If not specified, trace logs will be written to `$PREK_HOME/prek.log`

**[`--no-progress`](<#prek-cache-gc--no-progress>)**

Hide all progress outputs.

For example, spinners or progress bars.

**[`--quiet`](<#prek-cache-gc--quiet>), `-q`**

Use quiet output.

Repeating this option, e.g., `-qq`, will enable a silent mode in which prek will write no output to stdout.

May also be set with the `PREK_QUIET` environment variable.

**[`--refresh`](<#prek-cache-gc--refresh>)**

Refresh all cached data

**[`--verbose`](<#prek-cache-gc--verbose>), `-v`**

Use verbose output

**[`--version`](<#prek-cache-gc--version>), `-V`**

Display the prek version

### prek cache clean

Remove all prek cached data

### Usage

```text
prek cache clean [OPTIONS]
```

### Options

**[`--cd`](<#prek-cache-clean--cd>), `-C` *dir***

Change to directory before running

**[`--color`](<#prek-cache-clean--color>) *color***

Whether to use color in output

May also be set with the `PREK_COLOR` environment variable.

\[default: auto\]

Possible values:

- `auto`: Enables colored output only when the output is going to a terminal or TTY with support
- `always`: Enables colored output regardless of the detected environment
- `never`: Disables colored output

**[`--config`](<#prek-cache-clean--config>), `-c` *config***

Path to alternate config file

**[`--help`](<#prek-cache-clean--help>), `-h`**

Display the concise help for this command

**[`--log-file`](<#prek-cache-clean--log-file>) *log-file***

Write trace logs to the specified file. If not specified, trace logs will be written to `$PREK_HOME/prek.log`

**[`--no-progress`](<#prek-cache-clean--no-progress>)**

Hide all progress outputs.

For example, spinners or progress bars.

**[`--quiet`](<#prek-cache-clean--quiet>), `-q`**

Use quiet output.

Repeating this option, e.g., `-qq`, will enable a silent mode in which prek will write no output to stdout.

May also be set with the `PREK_QUIET` environment variable.

**[`--refresh`](<#prek-cache-clean--refresh>)**

Refresh all cached data

**[`--verbose`](<#prek-cache-clean--verbose>), `-v`**

Use verbose output

**[`--version`](<#prek-cache-clean--version>), `-V`**

Display the prek version

### prek cache size

Show the size of the prek cache

### Usage

```text
prek cache size [OPTIONS]
```

### Options

**[`--cd`](<#prek-cache-size--cd>), `-C` *dir***

Change to directory before running

**[`--color`](<#prek-cache-size--color>) *color***

Whether to use color in output

May also be set with the `PREK_COLOR` environment variable.

\[default: auto\]

Possible values:

- `auto`: Enables colored output only when the output is going to a terminal or TTY with support
- `always`: Enables colored output regardless of the detected environment
- `never`: Disables colored output

**[`--config`](<#prek-cache-size--config>), `-c` *config***

Path to alternate config file

**[`--help`](<#prek-cache-size--help>), `-h`**

Display the concise help for this command

**[`--human`](<#prek-cache-size--human>), `--human-readable`, `-H`**

Display the cache size in human-readable format (e.g., `1.2GiB` instead of raw bytes)

**[`--log-file`](<#prek-cache-size--log-file>) *log-file***

Write trace logs to the specified file. If not specified, trace logs will be written to `$PREK_HOME/prek.log`

**[`--no-progress`](<#prek-cache-size--no-progress>)**

Hide all progress outputs.

For example, spinners or progress bars.

**[`--output-format`](<#prek-cache-size--output-format>) *output-format***

Select the output format

\[default: auto\]

Possible values:

- `auto`: Display a human-readable size in terminals and raw bytes otherwise
- `human`: Display the cache size in a human-readable format
- `machine`: Display the cache size in raw bytes

**[`--quiet`](<#prek-cache-size--quiet>), `-q`**

Use quiet output.

Repeating this option, e.g., `-qq`, will enable a silent mode in which prek will write no output to stdout.

May also be set with the `PREK_QUIET` environment variable.

**[`--refresh`](<#prek-cache-size--refresh>)**

Refresh all cached data

**[`--verbose`](<#prek-cache-size--verbose>), `-v`**

Use verbose output

**[`--version`](<#prek-cache-size--version>), `-V`**

Display the prek version

## prek try-repo

Try hooks from a repository

### Usage

```text
prek try-repo [OPTIONS] <REPO> [HOOK|PROJECT]...
```

### Arguments

**[`REPO`](<#prek-try-repo--repo>)**

Repository to source hooks from

**[`HOOK|PROJECT`](<#prek-try-repo--includes>)**

Include the specified hooks or projects.

Supports flexible selector syntax:

- `hook-id`: Run all hooks with the specified ID across all projects
- `project-path/`: Run all hooks from the specified project
- `project-path:hook-id`: Run only the specified hook from the specified project

Can be specified multiple times to select multiple hooks/projects.

### Options

**[`--all-files`](<#prek-try-repo--all-files>), `-a`**

Run hooks on all tracked files in the repository

**[`--cd`](<#prek-try-repo--cd>), `-C` *dir***

Change to directory before running

**[`--color`](<#prek-try-repo--color>) *color***

Whether to use color in output

May also be set with the `PREK_COLOR` environment variable.

\[default: auto\]

Possible values:

- `auto`: Enables colored output only when the output is going to a terminal or TTY with support
- `always`: Enables colored output regardless of the detected environment
- `never`: Disables colored output

**[`--config`](<#prek-try-repo--config>), `-c` *config***

Path to alternate config file

**[`--directory`](<#prek-try-repo--directory>), `-d` *dir***

Run hooks on tracked files under the specified directory.

Paths are resolved relative to the current working directory after applying `--cd`. This option can be repeated and combined with `--files` and `--glob`.

**[`--dry-run`](<#prek-try-repo--dry-run>)**

Do not run the hooks, but print the hooks that would have been run

**[`--fail-fast`](<#prek-try-repo--fail-fast>)**

Stop running hooks after the first failure

**[`--files`](<#prek-try-repo--files>) *files***

Run hooks on the specified file paths.

Paths are resolved relative to the current working directory after applying `--cd`. They may be tracked or untracked. This option accepts multiple paths and can be combined with `--glob` and `--directory`.

**[`--from-ref`](<#prek-try-repo--from-ref>), `--source`, `-s` *from-ref***

The original ref in a `<from_ref>...<to_ref>` diff expression. Files changed in this diff will be run through the hooks

**[`--glob`](<#prek-try-repo--glob>) *pattern***

Run hooks on tracked files matching the specified glob pattern.

Patterns are matched against paths relative to the current working directory after applying `--cd`. Quote patterns to prevent shell expansion. This option can be repeated and combined with `--files` and `--directory`.

**[`--help`](<#prek-try-repo--help>), `-h`**

Display the concise help for this command

**[`--hide-status`](<#prek-try-repo--hide-status>) *status***

Hide hook reports with the specified final status.

Can be specified multiple times or as a comma-separated list. This does not change hook execution or exit codes. Overrides `hide_status` in project and user configuration.

Possible values:

- `passed`
- `failed`
- `skipped`

**[`--last-commit`](<#prek-try-repo--last-commit>)**

Run hooks against the last commit. Equivalent to `--from-ref HEAD~1 --to-ref HEAD`

**[`--log-file`](<#prek-try-repo--log-file>) *log-file***

Write trace logs to the specified file. If not specified, trace logs will be written to `$PREK_HOME/prek.log`

**[`--no-hide-status`](<#prek-try-repo--no-hide-status>)**

Show all hook reports, overriding `hide_status` in configuration

**[`--no-progress`](<#prek-try-repo--no-progress>)**

Hide all progress outputs.

For example, spinners or progress bars.

**[`--quiet`](<#prek-try-repo--quiet>), `-q`**

Use quiet output.

Repeating this option, e.g., `-qq`, will enable a silent mode in which prek will write no output to stdout.

May also be set with the `PREK_QUIET` environment variable.

**[`--refresh`](<#prek-try-repo--refresh>)**

Refresh all cached data

**[`--rev`](<#prek-try-repo--rev>), `--ref` *rev***

Manually select a rev to run against, otherwise the `HEAD` revision will be used

**[`--show-diff-on-failure`](<#prek-try-repo--show-diff-on-failure>)**

When hooks fail, run `git diff` directly afterward

**[`--skip`](<#prek-try-repo--skip>) *hook|project***

Skip the specified hooks or projects.

Supports flexible selector syntax:

- `hook-id`: Skip all hooks with the specified ID across all projects
- `project-path/`: Skip all hooks from the specified project
- `project-path:hook-id`: Skip only the specified hook from the specified project

Can be specified multiple times. Also accepts `PREK_SKIP` or `SKIP` environment variables (comma-delimited).

**[`--stage`](<#prek-try-repo--stage>), `--hook-stage` *stage***

The stage during which the hook is fired.

When specified, only hooks configured for that stage (for example `manual`, `pre-commit`, or `pre-push`) will run. When not specified, this command starts with hooks eligible for `pre-commit`. If no hook is selected and the command named hook IDs, those same IDs are matched again against hooks configured for `manual`.

Possible values:

- `manual`
- `commit-msg`
- `post-checkout`
- `post-commit`
- `post-merge`
- `post-rewrite`
- `pre-commit`
- `pre-merge-commit`
- `pre-push`
- `pre-rebase`
- `prepare-commit-msg`

**[`--to-ref`](<#prek-try-repo--to-ref>), `--origin`, `-o` *to-ref***

The destination ref in a `from_ref...to_ref` diff expression. Defaults to `HEAD` if `from_ref` is specified

**[`--verbose`](<#prek-try-repo--verbose>), `-v`**

Use verbose output

**[`--version`](<#prek-try-repo--version>), `-V`**

Display the prek version

## prek util

Run utility commands

### Usage

```text
prek util [OPTIONS] <COMMAND>
```

### Commands

**[`prek util identify`](<#prek-util-identify>)**

Show file identification tags

**[`prek util list-builtins`](<#prek-util-list-builtins>)**

List all built-in hooks bundled with prek

**[`prek util init-template-dir`](<#prek-util-init-template-dir>)**

Install Git shims in a directory intended for use with `git config init.templateDir`

**[`prek util yaml-to-toml`](<#prek-util-yaml-to-toml>)**

Convert a YAML configuration file to prek.toml

**[`prek util generate-shell-completion`](<#prek-util-generate-shell-completion>)**

Generate shell completion scripts

### prek util identify

Show file identification tags

### Usage

```text
prek util identify [OPTIONS] [PATH]...
```

### Arguments

**[`PATH`](<#prek-util-identify--paths>)**

The path(s) to the file(s) to identify

### Options

**[`--cd`](<#prek-util-identify--cd>), `-C` *dir***

Change to directory before running

**[`--color`](<#prek-util-identify--color>) *color***

Whether to use color in output

May also be set with the `PREK_COLOR` environment variable.

\[default: auto\]

Possible values:

- `auto`: Enables colored output only when the output is going to a terminal or TTY with support
- `always`: Enables colored output regardless of the detected environment
- `never`: Disables colored output

**[`--config`](<#prek-util-identify--config>), `-c` *config***

Path to alternate config file

**[`--help`](<#prek-util-identify--help>), `-h`**

Display the concise help for this command

**[`--log-file`](<#prek-util-identify--log-file>) *log-file***

Write trace logs to the specified file. If not specified, trace logs will be written to `$PREK_HOME/prek.log`

**[`--no-progress`](<#prek-util-identify--no-progress>)**

Hide all progress outputs.

For example, spinners or progress bars.

**[`--output-format`](<#prek-util-identify--output-format>) *output-format***

The output format

\[default: text\]

Possible values:

- `text`
- `json`

**[`--quiet`](<#prek-util-identify--quiet>), `-q`**

Use quiet output.

Repeating this option, e.g., `-qq`, will enable a silent mode in which prek will write no output to stdout.

May also be set with the `PREK_QUIET` environment variable.

**[`--refresh`](<#prek-util-identify--refresh>)**

Refresh all cached data

**[`--verbose`](<#prek-util-identify--verbose>), `-v`**

Use verbose output

**[`--version`](<#prek-util-identify--version>), `-V`**

Display the prek version

### prek util list-builtins

List all built-in hooks bundled with prek

### Usage

```text
prek util list-builtins [OPTIONS]
```

### Options

**[`--cd`](<#prek-util-list-builtins--cd>), `-C` *dir***

Change to directory before running

**[`--color`](<#prek-util-list-builtins--color>) *color***

Whether to use color in output

May also be set with the `PREK_COLOR` environment variable.

\[default: auto\]

Possible values:

- `auto`: Enables colored output only when the output is going to a terminal or TTY with support
- `always`: Enables colored output regardless of the detected environment
- `never`: Disables colored output

**[`--config`](<#prek-util-list-builtins--config>), `-c` *config***

Path to alternate config file

**[`--help`](<#prek-util-list-builtins--help>), `-h`**

Display the concise help for this command

**[`--log-file`](<#prek-util-list-builtins--log-file>) *log-file***

Write trace logs to the specified file. If not specified, trace logs will be written to `$PREK_HOME/prek.log`

**[`--no-progress`](<#prek-util-list-builtins--no-progress>)**

Hide all progress outputs.

For example, spinners or progress bars.

**[`--output-format`](<#prek-util-list-builtins--output-format>) *output-format***

The output format

\[default: text\]

Possible values:

- `text`
- `json`

**[`--quiet`](<#prek-util-list-builtins--quiet>), `-q`**

Use quiet output.

Repeating this option, e.g., `-qq`, will enable a silent mode in which prek will write no output to stdout.

May also be set with the `PREK_QUIET` environment variable.

**[`--refresh`](<#prek-util-list-builtins--refresh>)**

Refresh all cached data

**[`--verbose`](<#prek-util-list-builtins--verbose>), `-v`**

Use verbose output

**[`--version`](<#prek-util-list-builtins--version>), `-V`**

Display the prek version

### prek util init-template-dir

Install Git shims in a directory intended for use with `git config init.templateDir`

### Usage

```text
prek util init-template-dir [OPTIONS] <DIRECTORY>
```

### Arguments

**[`DIRECTORY`](<#prek-util-init-template-dir--directory>)**

The directory in which to write the Git shim

### Options

**[`--cd`](<#prek-util-init-template-dir--cd>), `-C` *dir***

Change to directory before running

**[`--color`](<#prek-util-init-template-dir--color>) *color***

Whether to use color in output

May also be set with the `PREK_COLOR` environment variable.

\[default: auto\]

Possible values:

- `auto`: Enables colored output only when the output is going to a terminal or TTY with support
- `always`: Enables colored output regardless of the detected environment
- `never`: Disables colored output

**[`--config`](<#prek-util-init-template-dir--config>), `-c` *config***

Path to alternate config file

**[`--help`](<#prek-util-init-template-dir--help>), `-h`**

Display the concise help for this command

**[`--hook-type`](<#prek-util-init-template-dir--hook-type>), `-t` *hook-type***

Which Git shim(s) to install.

Specifies which Git hook type(s) you want to install shims for. Can be specified multiple times to install shims for multiple hook types.

If not specified, uses `default_install_hook_types` from the config file, or defaults to `pre-commit` if that is also not set.

Possible values:

- `commit-msg`
- `post-checkout`
- `post-commit`
- `post-merge`
- `post-rewrite`
- `pre-commit`
- `pre-merge-commit`
- `pre-push`
- `pre-rebase`
- `prepare-commit-msg`

**[`--log-file`](<#prek-util-init-template-dir--log-file>) *log-file***

Write trace logs to the specified file. If not specified, trace logs will be written to `$PREK_HOME/prek.log`

**[`--no-allow-missing-config`](<#prek-util-init-template-dir--no-allow-missing-config>)**

Assume cloned repos should have a `pre-commit` config

**[`--no-progress`](<#prek-util-init-template-dir--no-progress>)**

Hide all progress outputs.

For example, spinners or progress bars.

**[`--quiet`](<#prek-util-init-template-dir--quiet>), `-q`**

Use quiet output.

Repeating this option, e.g., `-qq`, will enable a silent mode in which prek will write no output to stdout.

May also be set with the `PREK_QUIET` environment variable.

**[`--refresh`](<#prek-util-init-template-dir--refresh>)**

Refresh all cached data

**[`--verbose`](<#prek-util-init-template-dir--verbose>), `-v`**

Use verbose output

**[`--version`](<#prek-util-init-template-dir--version>), `-V`**

Display the prek version

### prek util yaml-to-toml

Convert a YAML configuration file to prek.toml

### Usage

```text
prek util yaml-to-toml [OPTIONS] [CONFIG]
```

### Arguments

**[`CONFIG`](<#prek-util-yaml-to-toml--input>)**

The YAML configuration file to convert. If omitted, discovers `.pre-commit-config.yaml` or `.pre-commit-config.yml` in the current directory

### Options

**[`--cd`](<#prek-util-yaml-to-toml--cd>), `-C` *dir***

Change to directory before running

**[`--color`](<#prek-util-yaml-to-toml--color>) *color***

Whether to use color in output

May also be set with the `PREK_COLOR` environment variable.

\[default: auto\]

Possible values:

- `auto`: Enables colored output only when the output is going to a terminal or TTY with support
- `always`: Enables colored output regardless of the detected environment
- `never`: Disables colored output

**[`--config`](<#prek-util-yaml-to-toml--config>), `-c` *config***

Path to alternate config file

**[`--force`](<#prek-util-yaml-to-toml--force>)**

Overwrite the output file if it already exists

**[`--help`](<#prek-util-yaml-to-toml--help>), `-h`**

Display the concise help for this command

**[`--log-file`](<#prek-util-yaml-to-toml--log-file>) *log-file***

Write trace logs to the specified file. If not specified, trace logs will be written to `$PREK_HOME/prek.log`

**[`--no-progress`](<#prek-util-yaml-to-toml--no-progress>)**

Hide all progress outputs.

For example, spinners or progress bars.

**[`--output`](<#prek-util-yaml-to-toml--output>), `-o` *output***

Path to write the generated prek.toml file. Defaults to `prek.toml` in the same directory as the input file

**[`--quiet`](<#prek-util-yaml-to-toml--quiet>), `-q`**

Use quiet output.

Repeating this option, e.g., `-qq`, will enable a silent mode in which prek will write no output to stdout.

May also be set with the `PREK_QUIET` environment variable.

**[`--refresh`](<#prek-util-yaml-to-toml--refresh>)**

Refresh all cached data

**[`--verbose`](<#prek-util-yaml-to-toml--verbose>), `-v`**

Use verbose output

**[`--version`](<#prek-util-yaml-to-toml--version>), `-V`**

Display the prek version

### prek util generate-shell-completion

Generate shell completion scripts.

Load the generated script in your shell configuration to enable completion.

### Usage

```text
prek util generate-shell-completion [OPTIONS] <SHELL>
```

### Arguments

**[`SHELL`](<#prek-util-generate-shell-completion--shell>)**

The shell to generate the completion script for

### Options

**[`--cd`](<#prek-util-generate-shell-completion--cd>), `-C` *dir***

Change to directory before running

**[`--color`](<#prek-util-generate-shell-completion--color>) *color***

Whether to use color in output

May also be set with the `PREK_COLOR` environment variable.

\[default: auto\]

Possible values:

- `auto`: Enables colored output only when the output is going to a terminal or TTY with support
- `always`: Enables colored output regardless of the detected environment
- `never`: Disables colored output

**[`--config`](<#prek-util-generate-shell-completion--config>), `-c` *config***

Path to alternate config file

**[`--help`](<#prek-util-generate-shell-completion--help>), `-h`**

Display the concise help for this command

**[`--log-file`](<#prek-util-generate-shell-completion--log-file>) *log-file***

Write trace logs to the specified file. If not specified, trace logs will be written to `$PREK_HOME/prek.log`

**[`--no-progress`](<#prek-util-generate-shell-completion--no-progress>)**

Hide all progress outputs.

For example, spinners or progress bars.

**[`--quiet`](<#prek-util-generate-shell-completion--quiet>), `-q`**

Use quiet output.

Repeating this option, e.g., `-qq`, will enable a silent mode in which prek will write no output to stdout.

May also be set with the `PREK_QUIET` environment variable.

**[`--refresh`](<#prek-util-generate-shell-completion--refresh>)**

Refresh all cached data

**[`--verbose`](<#prek-util-generate-shell-completion--verbose>), `-v`**

Use verbose output

**[`--version`](<#prek-util-generate-shell-completion--version>), `-V`**

Display the prek version

## prek self

Manage the prek installation

### Usage

```text
prek self [OPTIONS] <COMMAND>
```

### Commands

**[`prek self update`](<#prek-self-update>)**

Update prek

### prek self update

Update prek

### Usage

```text
prek self update [OPTIONS] [TARGET_VERSION]
```

### Arguments

**[`TARGET_VERSION`](<#prek-self-update--target_version>)**

Update to the specified version. If not provided, prek will update to the latest version

### Options

**[`--cd`](<#prek-self-update--cd>), `-C` *dir***

Change to directory before running

**[`--color`](<#prek-self-update--color>) *color***

Whether to use color in output

May also be set with the `PREK_COLOR` environment variable.

\[default: auto\]

Possible values:

- `auto`: Enables colored output only when the output is going to a terminal or TTY with support
- `always`: Enables colored output regardless of the detected environment
- `never`: Disables colored output

**[`--config`](<#prek-self-update--config>), `-c` *config***

Path to alternate config file

**[`--help`](<#prek-self-update--help>), `-h`**

Display the concise help for this command

**[`--log-file`](<#prek-self-update--log-file>) *log-file***

Write trace logs to the specified file. If not specified, trace logs will be written to `$PREK_HOME/prek.log`

**[`--no-progress`](<#prek-self-update--no-progress>)**

Hide all progress outputs.

For example, spinners or progress bars.

**[`--quiet`](<#prek-self-update--quiet>), `-q`**

Use quiet output.

Repeating this option, e.g., `-qq`, will enable a silent mode in which prek will write no output to stdout.

May also be set with the `PREK_QUIET` environment variable.

**[`--refresh`](<#prek-self-update--refresh>)**

Refresh all cached data

**[`--token`](<#prek-self-update--token>) *token***

A GitHub token for authentication. A token is not required but can be used to reduce the chance of encountering rate limits

May also be set with the `GITHUB_TOKEN` environment variable.

**[`--verbose`](<#prek-self-update--verbose>), `-v`**

Use verbose output

**[`--version`](<#prek-self-update--version>), `-V`**

Display the prek version
