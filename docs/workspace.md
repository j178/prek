# Workspace Mode

`prek` supports a powerful workspace mode that allows you to manage multiple projects with their own pre-commit configurations within a single repository. This is particularly useful for monorepos or projects with complex directory structures.

## Overview

A **workspace** is a directory structure that contains:

- A root `.pre-commit-config.yaml` file
- Zero or more nested `.pre-commit-config.yaml` files in subdirectories

Each directory containing a `.pre-commit-config.yaml` file is considered a **project**. Projects can be nested infinitely deep.

## Discovery

When you run `prek run` without the `--config` option, `prek` automatically discovers the workspace:

1. **Find workspace root**: Starting from the current working directory, `prek` walks up the directory tree until it finds a `.pre-commit-config.yaml` file. This becomes the workspace root.

2. **Discover all projects**: From the workspace root, `prek` recursively searches all subdirectories for additional `.pre-commit-config.yaml` files. Each one becomes a separate project.

3. **Git repository boundary**: The search stops at the git repository root (`.git` directory) to avoid including unrelated projects.

**Note**:

- The workspace root is not necessarily the same as the git repository root, a workspace can exist within a subdirectory of a git repository.

- The current working directory determines the workspace root discovery. `prek` starts searching from your current location and stops at the first `.pre-commit-config.yaml` file found while traversing up the directory tree. Running from different directories may discover different workspace roots. Use `prek -C <dir>` to change the working directory before execution.

- Directories beginning with a dot (e.g. `.hidden`) are ignored during project discovery.

- By default, `prek` respects `.gitignore` files during workspace discovery. This means any directories or files excluded by `.gitignore`, `.git/info/exclude`, or your global gitignore configuration will automatically be excluded from project discovery. This prevents `prek` from discovering workspaces in ignored directories like `node_modules`, `target`, or `.venv`.

- For additional control, `prek` also supports reading `.prekignore` files (following the same syntax rules as `.gitignore`) to exclude specific directories from workspace discovery beyond what's in `.gitignore`. Like `.gitignore`, `.prekignore` files can be placed anywhere in the workspace and apply to their directory and all subdirectories. This works similarly to the `--skip` option but is configured via files.

## Project Organization

### Example Structure

```text
my-monorepo/
├── .pre-commit-config.yaml          # Workspace root config
├── .git/
├── docs/
│   └── .pre-commit-config.yaml      # Nested project
├── src/
│   ├── .pre-commit-config.yaml      # Nested project
│   └── backend/
│       └── .pre-commit-config.yaml  # Deeply nested project
└── frontend/
    └── .pre-commit-config.yaml      # Nested project
```

In this example:

- `my-monorepo/` is the workspace root
- `docs/`, `src/`, `src/backend/`, and `frontend/` are individual projects
- Each project has its own `.pre-commit-config.yaml` file

## Execution Model

### File Collection

When running in workspace mode:

1. **Collect all files**: `prek` collects all files within the workspace root directory
2. **Apply global filters**: Files are filtered based on include/exclude patterns from the workspace root config
3. **Distribute to projects**: Each project receives a subset of files based on its location

#### File Visibility Constraints

**Important**: Each project can only see and process files within its own directory tree. This is a fundamental design principle of workspace mode that ensures proper isolation between projects.

A hook defined in `frontend/.pre-commit-config.yaml` can only match files under the `frontend/` directory—it cannot reference files from sibling directories like `backend/`. If hooks need to reference files across multiple projects, move the hook configuration to a common ancestor directory (e.g., the workspace root).

### Hook Execution

For each project:

1. **Scope to project directory**: Hooks run within their project's root directory
2. **Filter files**: Only files within the project's directory tree are passed to its hooks
3. **Independent execution**: Each project's hooks run independently with their own environment

### Execution Order

Projects are executed from **deepest to shallowest**:

1. `src/backend/` (deepest)
2. `src/`
3. `docs/`
4. `frontend/`
5. `./` (root, last)

This ensures that more specific configurations (deeper projects) take precedence over general ones.

### File Processing Behavior

**By default**, files in subprojects will be processed multiple times - once for each project in the hierarchy that contains them. For example, a file in `src/backend/` will be checked by hooks in `src/backend/`, then `src/`, then the workspace root.

**To isolate a project**, you can set `orphan: true` in its configuration. When enabled, files in this project are "consumed" by it and will not be processed by parent projects:

```yaml
# src/backend/.pre-commit-config.yaml
orphan: true

repos:
  - repo: https://github.com/astral-sh/ruff-pre-commit
    rev: v0.8.4
    hooks:
      - id: ruff
```

With this option:
- Files in `src/backend/` are processed **only** by hooks in `src/backend/`
- Files in `src/` (but not in `src/backend/`) are processed by hooks in `src/` and the workspace root
- Files in the root (but not in subdirectories with configs) are processed by hooks in the root

This can be useful to avoid redundant processing in monorepos with nested project structures or to completely isolate a subproject from parent configurations.

### Example Output

When running `prek run` on the example structure above, you might see output like this:

```console
$ prek run
Running hooks for `src/backend`:
check python ast.........................................................Passed
check for merge conflicts................................................Passed
black....................................................................Passed
isort....................................................................Passed

Running hooks for `docs`:
Markdownlint.........................................(unimplemented yet)Skipped

Running hooks for `frontend`:
prettier.................................................................Passed

Running hooks for `src`:
isort....................................................................Passed
mypy.....................................................................Passed
check python ast.........................................................Passed
check docstring is first.................................................Passed

Running hooks for `.`:
fix end of files.........................................................Passed
check yaml...............................................................Passed
check for added large files..............................................Passed
trim trailing whitespace.................................................Passed
check for merge conflicts................................................Passed
```

Notice how:

- Files in `src/backend/` are processed by both the `src/backend/` project and the `src/` project
- Each project runs in its own working directory
- The workspace root processes all files in the entire workspace
- Projects are executed from deepest to shallowest as described in the execution order

#### Orphan Projects and Selectors

When you combine `orphan: true` with selectors such as `--skip`, remember that orphans keep the files they cover. Even if you skip an orphan project (for example via `--skip src/backend/`), that project still claims ownership of the files under its directory. Those files will not fall back to parent projects, so you can disable or precisely target orphaned projects without reintroducing duplicate processing upstream.

## Command Line Usage

```bash
# Run from current directory, auto-discover workspace
prek run

# Run specific hook across all projects
prek run black

# Run from specific directory
cd src/backend && prek run

# Use -C option to change directory automatically
prek run -C src/backend
```

The `-C <dir>` or `--cd <dir>` option automatically changes to the specified directory before running, allowing you to target specific projects from any location in the workspace.

**Note**: When using `prek install`, only the workspace root configuration's `default_install_hook_types` will be honored. Nested project configurations are not considered during installation.

## Project and Hook Selection

In workspace mode, you can selectively run hooks from specific projects or skip certain projects/hooks using flexible selector syntax.

### Selector Syntax

The selector syntax has three different forms:

1. **`<hook-id>`**: Matches all hooks with the given ID across all projects.
2. **`<project-path>/`**: Matches all hooks from the specified project and its subprojects.
3. **`<project-path>:<hook-id>`**: Matches only the specified hook from the specified project.

Selectors can be used to select specific hooks or projects, and combined with `--skip` to exclude certain hooks or projects.

**Note**: `<project-path>` can be a relative path, which is then resolved relative to the current working directory.
Note that the trailing slash `/` in a `<project-path>` is important, if a selector does not contain a slash, it is interpreted as a hook ID.

### Running Specific Hooks or Projects

```bash
# Run all hooks with a specific ID across all projects
prek run <hook-id>

# Run only hooks from a specific project
prek run <project-path>/

# Run only hooks with a specific ID from a specific project
prek run <project-path>:<hook-id>
```

**Examples:**

```bash
# Run all 'black' hooks across all projects
prek run black

# Run all hooks from the 'frontend' project
prek run frontend/

# Run only the 'lint' hook from the 'frontend' project
prek run frontend:lint

# Run the 'lint' from 'frontend' and 'black' from 'src/backend'
prek run frontend:lint src/backend:black
```

### Skipping Projects or Hooks

You can skip specific projects or hooks using the `--skip` option, with the same syntax as for selecting projects or hooks.

**Alternative**: You can also create `.prekignore` files (using `.gitignore` syntax) anywhere in the workspace to permanently exclude directories from project discovery during workspace setup. Note that `.gitignore` files are already respected by default, so `.prekignore` is only needed for excluding additional directories beyond what's in `.gitignore`.

```bash
# Skip all hooks from a specific project
prek run --skip <project-path>/

# Skip specific hooks within a selected project
prek run <project-path>/ --skip <subproject-path>/

# Skip all hooks with a specific ID across all projects
prek run --skip <hook-id>
```

**Examples:**

```bash
# Run all hooks except those from the 'frontend' project
prek run --skip frontend/

# Run hooks from 'frontend' but skip 'frontend/docs'
prek run frontend/ --skip frontend/docs

# Run hooks from 'frontend' but skip 'frontend/docs' and 'frontend:lint'
prek run frontend/ --skip frontend/docs --skip frontend:lint

# Run all hooks except 'black' and 'markdownlint' hooks
prek run --skip black --skip markdownlint
```

**Note**: Selecting a project includes all its subprojects unless explicitly skipped. Skipping a project also skips all its subprojects.

**Note**: The `PREK_SKIP` or `SKIP` environment variable can be used as an alternative to `--skip`. Multiple values should be comma-delimited:

```bash
# Skip 'frontend' and 'tests' projects
PREK_SKIP=frontend/,tests prek run

# Skip 'frontend/docs' project and 'src/backend:lint' hook
SKIP=frontend/docs,src/backend:lint prek run
```

Precedence rules for `--skip` command line options and environment variables are: `--skip` > `PREK_SKIP` > `SKIP`.

### Advanced Examples

```bash
# Run 'lint' hooks from all projects except 'tests'
prek run lint --skip tests

# Run all hooks from 'src' and 'docs' but skip 'src/legacy'
prek run src/ docs/ --skip src/legacy

# Run 'format' hooks only from Python projects
prek run python:format
```

## Single Config Mode

When you specify a configuration file using the `-c` or `--config` parameter, workspace mode is disabled and only the specified configuration file is used. This mode provides traditional pre-commit behavior similar to the original pre-commit tool.

In single config mode:

- **No workspace discovery**: Only the explicitly specified configuration file is used
- **Single execution context**: All hooks run from the git repository root directory
- **Global file scope**: All files in the git repository are passed to all hooks
- **No project isolation**: Hooks don't have access to project-specific working directories

### Usage Examples

```bash
# Disable workspace mode, use specific config
prek run --config .pre-commit-config.yaml

# Use config from a subdirectory
prek run --config src/.pre-commit-config.yaml

# Short form using -c
prek run -c docs/.pre-commit-config.yaml
```

### Key Differences: Workspace vs Single Config

| Feature | Workspace Mode | Single Config Mode |
|---------|----------------|-------------------|
| **Discovery** | Auto-discovers all `.pre-commit-config.yaml` files | Uses single specified config file |
| **Working Directory** | Uses workspace root | Uses git repository root |
| **File Scope** | All files in workspace | All files in git repo |
| **Hook Scope** | Project-specific file filtering | All files pass to all hooks |
| **Execution Context** | Each project runs in its own directory | All hooks run from git root |
| **Configuration** | Multiple configs | Single config file only |

### Migration from Single Config

To migrate an existing single-config setup to workspace mode:

1. **Create workspace root**: Move existing `.pre-commit-config.yaml` to repository root
2. **Add project configs**: Create `.pre-commit-config.yaml` in subdirectories as needed
3. **Update file patterns**: Adjust `files`/`exclude` patterns to be project-relative
4. **Test execution**: Verify hooks run in correct directories with correct file sets

## Workspace Cache

To improve performance in large monorepos, `prek` introduces a workspace cache mechanism. The workspace cache stores the results of project discovery, so repeated runs are much faster.

- The cache is automatically used by default. You don't need to do anything for it to work.
- If you make changes to `.pre-commit-config.yaml` files, remove projects, or otherwise change the workspace structure, `prek` will usually detect this and refresh the cache automatically.
- If you add a new `.pre-commit-config.yaml` to your workspace, `prek` may not detect it immediately, try running with `--refresh` to ensure the cache is up to date.

```bash
prek run --refresh
```

This will clear and rebuild the workspace cache before running hooks.

## Behavior Changes in Workspace Mode

When running in workspace mode, there are a few changes to the output format and behavior compared to single-config mode:

1. Hook output is grouped by project, with a header indicating which project is currently running.
2. Skipped hooks are not shown at all in the output, previously they were listed as "Skipped".

The workspace mode provides powerful organization capabilities while maintaining backward compatibility with existing single-config workflows.
