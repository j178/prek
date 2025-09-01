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

## Project Organization

### Example Structure

```
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
5. `my-monorepo/` (root, last)

This ensures that more specific configurations (deeper projects) take precedence over general ones.

## Command Line Usage

### Workspace Mode (Default)

```bash
# Run from current directory, auto-discover workspace
prek run

# Run specific hook across all projects
prek run black

# Run from specific directory
cd src/backend && prek run
```

#### TODO: Directory Change Option

- Add a `-C <dir>` option to `prek run` to automatically change to the directory before running
- This would allow running workspace commands from any location while targeting a specific directory
- Example: `prek run -C src/backend` would change to `src/backend` before executing

#### TODO: Hook ID Prefix Filtering

- Add project prefix to hook IDs to identify which project they belong to
- Allow filtering hooks by ID prefix to run only hooks from specific projects
- Example: `prek run docs/black` would run only the `black` hook from the `docs/` project

### Single Config Mode

```bash
# Disable workspace mode, use specific config
prek run --config .pre-commit-config.yaml

# This runs from git root, not workspace root
# Only uses the specified config file
```

## Key Differences: Workspace vs Single Config

| Feature | Workspace Mode | Single Config Mode |
|---------|----------------|-------------------|
| **Discovery** | Auto-discovers all `.pre-commit-config.yaml` files | Uses single specified config file |
| **Working Directory** | Uses workspace root | Uses git repository root |
| **File Scope** | All files in workspace | All files in git repo |
| **Hook Scope** | Project-specific file filtering | All files pass to all hooks |
| **Execution Context** | Each project runs in its own directory | All hooks run from git root |
| **Configuration** | Multiple configs, inheritance possible | Single config file only |

### Debugging

```bash
# See which projects were discovered
prek run -vvv

# Check file collection for specific project
cd project/dir && prek run -vvv
```

## Migration from Single Config

To migrate an existing single-config setup to workspace mode:

1. **Create workspace root**: Move existing `.pre-commit-config.yaml` to repository root
2. **Add project configs**: Create `.pre-commit-config.yaml` in subdirectories as needed
3. **Update file patterns**: Adjust `files`/`exclude` patterns to be project-relative
4. **Test execution**: Verify hooks run in correct directories with correct file sets

The workspace mode provides powerful organization capabilities while maintaining backward compatibility with existing single-config workflows.
