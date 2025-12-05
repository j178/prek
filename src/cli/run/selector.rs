use std::borrow::Cow;
use std::fmt::Display;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::hook::Hook;
use crate::warn_user;

use anyhow::anyhow;
use itertools::Itertools;
use path_clean::PathClean;
use prek_consts::env_vars::EnvVars;
use rustc_hash::FxHashSet;
use tracing::trace;

#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error("Invalid selector: `{selector}`")]
    InvalidSelector {
        selector: String,
        #[source]
        source: anyhow::Error,
    },

    #[error("Invalid project path: `{path}`")]
    InvalidPath {
        path: String,
        #[source]
        source: anyhow::Error,
    },
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum SelectorSource {
    CliArg,
    CliFlag(&'static str),
    EnvVar(&'static str),
}

#[derive(Debug, Clone)]
pub(crate) enum SelectorExpr {
    HookId(String),
    ProjectPrefix(PathBuf),
    ProjectHook {
        project_path: PathBuf,
        hook_id: String,
    },
}

#[derive(Debug, Clone)]
pub(crate) struct Selector {
    source: SelectorSource,
    original: String,
    expr: SelectorExpr,
}

impl Display for Selector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.expr {
            SelectorExpr::HookId(hook_id) => write!(f, "{hook_id}"),
            SelectorExpr::ProjectPrefix(project_path) => {
                if project_path.as_os_str().is_empty() {
                    write!(f, "./")
                } else {
                    write!(f, "{}/", project_path.display())
                }
            }
            SelectorExpr::ProjectHook {
                project_path,
                hook_id,
            } => {
                if project_path.as_os_str().is_empty() {
                    write!(f, ".:{hook_id}")
                } else {
                    write!(f, "{}:{hook_id}", project_path.display())
                }
            }
        }
    }
}

impl Selector {
    pub(crate) fn as_flag(&self) -> Cow<'_, str> {
        match &self.source {
            SelectorSource::CliArg => Cow::Borrowed(&self.original),
            SelectorSource::CliFlag(flag) => Cow::Owned(format!("{}={}", flag, self.original)),
            SelectorSource::EnvVar(var) => Cow::Owned(format!("{}={}", var, self.original)),
        }
    }

    pub(crate) fn as_normalized_flag(&self) -> String {
        match &self.source {
            SelectorSource::CliArg => self.to_string(),
            SelectorSource::CliFlag(flag) => format!("{flag}={self}"),
            SelectorSource::EnvVar(var) => format!("{var}={self}"),
        }
    }

    pub(crate) fn source(&self) -> &SelectorSource {
        &self.source
    }

    pub(crate) fn kind_str(&self) -> &'static str {
        match &self.expr {
            SelectorExpr::HookId(_) | SelectorExpr::ProjectHook { .. } => "hooks",
            SelectorExpr::ProjectPrefix(_) => "projects",
        }
    }
}

impl Selector {
    pub(crate) fn matches_hook(&self, hook: &Hook) -> bool {
        match &self.expr {
            SelectorExpr::HookId(hook_id) => {
                // For bare hook IDs, check if it matches the hook
                &hook.id == hook_id || &hook.alias == hook_id
            }
            SelectorExpr::ProjectPrefix(project_path) => {
                // For project paths, check if the hook belongs to that project.
                hook.project().relative_path().starts_with(project_path)
            }
            SelectorExpr::ProjectHook {
                project_path,
                hook_id,
            } => {
                // For project:hook syntax, check both
                (&hook.id == hook_id || &hook.alias == hook_id)
                    && project_path == hook.project().relative_path()
            }
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Selectors {
    includes: Vec<Selector>,
    skips: Vec<Selector>,
    usage: Arc<Mutex<SelectorUsage>>,
}

impl Selectors {
    /// Load include and skip selectors from CLI args and environment variables.
    pub(crate) fn load(
        includes: &[String],
        skips: &[String],
        workspace_root: &Path,
    ) -> Result<Selectors, Error> {
        let includes = includes
            .iter()
            .unique()
            .map(|selector| {
                parse_single_selector(
                    selector,
                    workspace_root,
                    SelectorSource::CliArg,
                    RealFileSystem,
                )
            })
            .collect::<Result<Vec<_>, _>>()?;

        trace!(
            "Include selectors: `{}`",
            includes
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        );

        let skips = load_skips(skips, workspace_root, RealFileSystem)?;

        trace!(
            "Skip selectors: `{}`",
            skips
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        );

        Ok(Self {
            includes,
            skips,
            usage: Arc::default(),
        })
    }

    pub(crate) fn includes(&self) -> &[Selector] {
        &self.includes
    }

    pub(crate) fn skips(&self) -> &[Selector] {
        &self.skips
    }

    pub(crate) fn has_project_selectors(&self) -> bool {
        self.includes.iter().any(|include| {
            matches!(
                include.expr,
                SelectorExpr::ProjectPrefix(_) | SelectorExpr::ProjectHook { .. }
            )
        })
    }

    pub(crate) fn includes_only_hook_targets(&self) -> bool {
        !self.includes.is_empty()
            && self.includes.iter().all(|s| {
                matches!(
                    s.expr,
                    SelectorExpr::HookId(_) | SelectorExpr::ProjectHook { .. }
                )
            })
    }

    /// Check if a hook matches any of the selection criteria.
    pub(crate) fn matches_hook(&self, hook: &Hook) -> bool {
        let mut usage = self.usage.lock().unwrap();

        // Always check every selector to track usage
        let mut skipped = false;
        for (idx, skip) in self.skips.iter().enumerate() {
            if skip.matches_hook(hook) {
                usage.use_skip(idx);
                skipped = true;
            }
        }
        if skipped {
            return false;
        }

        if self.includes.is_empty() {
            return true; // No `includes` mean all hooks are included
        }

        let mut included = false;
        for (idx, include) in self.includes.iter().enumerate() {
            if include.matches_hook(hook) {
                usage.use_include(idx);
                included = true;
            }
        }
        included
    }

    pub(crate) fn matches_hook_id(&self, hook_id: &str) -> bool {
        let mut usage = self.usage.lock().unwrap();

        // Always check every selector to track usage
        let mut skipped = false;
        for (idx, skip) in self.skips.iter().enumerate() {
            if let SelectorExpr::HookId(id) = &skip.expr {
                if id == hook_id {
                    usage.use_skip(idx);
                    skipped = true;
                }
            }
        }
        if skipped {
            return false;
        }

        if self.includes.is_empty() {
            return true; // No `includes` mean all hooks are included
        }

        let mut included = false;
        for (idx, include) in self.includes.iter().enumerate() {
            if let SelectorExpr::HookId(id) = &include.expr {
                if id == hook_id {
                    usage.use_include(idx);
                    included = true;
                }
            }
        }
        included
    }

    pub(crate) fn matches_path(&self, path: &Path) -> bool {
        let mut usage = self.usage.lock().unwrap();

        let mut skipped = false;
        for (idx, skip) in self.skips.iter().enumerate() {
            if let SelectorExpr::ProjectPrefix(project_path) = &skip.expr {
                if path.starts_with(project_path) {
                    usage.use_skip(idx);
                    skipped = true;
                }
            }
        }
        if skipped {
            return false;
        }

        // If no project prefix selectors are present, all paths are included
        if !self
            .includes
            .iter()
            .any(|include| matches!(include.expr, SelectorExpr::ProjectPrefix(_)))
        {
            return true;
        }

        let mut included = false;
        for (idx, include) in self.includes.iter().enumerate() {
            if let SelectorExpr::ProjectPrefix(project_path) = &include.expr {
                if path.starts_with(project_path) {
                    usage.use_include(idx);
                    included = true;
                }
            }
        }
        included
    }

    pub(crate) fn report_unused(&self) {
        let usage = self.usage.lock().unwrap();
        usage.report_unused(self);
    }
}

#[derive(Default, Debug)]
struct SelectorUsage {
    used_includes: FxHashSet<usize>,
    used_skips: FxHashSet<usize>,
}

impl SelectorUsage {
    fn use_include(&mut self, idx: usize) {
        self.used_includes.insert(idx);
    }

    fn use_skip(&mut self, idx: usize) {
        self.used_skips.insert(idx);
    }

    fn report_unused(&self, selectors: &Selectors) {
        let unused = selectors
            .includes
            .iter()
            .enumerate()
            .filter(|(idx, _)| !self.used_includes.contains(idx))
            .chain(
                selectors
                    .skips
                    .iter()
                    .enumerate()
                    .filter(|(idx, _)| !self.used_skips.contains(idx)),
            )
            .collect::<Vec<_>>();

        match unused.as_slice() {
            [] => {}
            [(_, selector)] => {
                let flag = selector.as_flag();
                let normalized = selector.as_normalized_flag();
                if flag == normalized {
                    warn_user!(
                        "selector `{flag}` did not match any {}",
                        selector.kind_str()
                    );
                } else {
                    warn_user!(
                        "selector `{flag}` ({}) did not match any {}",
                        format!("normalized to `{normalized}`").dimmed(),
                        selector.kind_str()
                    );
                }
            }
            _ => {
                let warning = unused
                    .iter()
                    .map(|(_, sel)| {
                        let flag = sel.as_flag();
                        let normalized = sel.as_normalized_flag();
                        if flag == normalized {
                            format!("  - `{flag}`")
                        } else {
                            format!(
                                "  - `{flag}` ({})",
                                format!("normalized to `{normalized}`").dimmed()
                            )
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n");

                warn_user!("the following selectors did not match any hooks or projects:");
                anstream::eprintln!("{warning}");
            }
        }
    }
}

/// Parse a single selector string into a Selection enum.
fn parse_single_selector<FS: FileSystem>(
    input: &str,
    workspace_root: &Path,
    source: SelectorSource,
    fs: FS,
) -> Result<Selector, Error> {
    if input.chars().filter(|&c| c == ':').count() > 1 {
        return Err(Error::InvalidSelector {
            selector: input.to_string(),
            source: anyhow!("only one ':' is allowed"),
        });
    }

    // Handle explicit hook ID with : prefix
    if let Some(hook_id) = input.strip_prefix(':') {
        if hook_id.is_empty() {
            return Err(Error::InvalidSelector {
                selector: input.to_string(),
                source: anyhow!("hook ID part is empty"),
            });
        }
        return Ok(Selector {
            source,
            original: input.to_string(),
            expr: SelectorExpr::HookId(hook_id.to_string()),
        });
    }

    // Handle `project:hook` syntax
    if let Some((project_path, hook_id)) = input.split_once(':') {
        if project_path.is_empty() {
            return Err(Error::InvalidSelector {
                selector: input.to_string(),
                source: anyhow!("project path part is empty"),
            });
        }
        if hook_id.is_empty() {
            return Err(Error::InvalidSelector {
                selector: input.to_string(),
                source: anyhow!("hook ID part is empty"),
            });
        }

        let project_path = normalize_path(project_path, workspace_root, fs).map_err(|e| {
            Error::InvalidSelector {
                selector: input.to_string(),
                source: anyhow!(e),
            }
        })?;

        return Ok(Selector {
            source,
            original: input.to_string(),
            expr: SelectorExpr::ProjectHook {
                project_path,
                hook_id: hook_id.to_string(),
            },
        });
    }

    // Handle project paths
    if input == "." || input.contains('/') {
        let project_path =
            normalize_path(input, workspace_root, fs).map_err(|e| Error::InvalidSelector {
                selector: input.to_string(),
                source: anyhow!(e),
            })?;

        return Ok(Selector {
            source,
            original: input.to_string(),
            expr: SelectorExpr::ProjectPrefix(project_path),
        });
    }

    // Ambiguous case: treat as hook ID for backward compatibility
    if input.is_empty() {
        return Err(Error::InvalidSelector {
            selector: input.to_string(),
            source: anyhow!("cannot be empty"),
        });
    }
    Ok(Selector {
        source,
        original: input.to_string(),
        expr: SelectorExpr::HookId(input.to_string()),
    })
}

/// Trait to abstract filesystem operations for easier testing.
pub trait FileSystem: Copy {
    fn absolute<P: AsRef<Path>>(&self, path: P) -> std::io::Result<PathBuf>;
}

#[derive(Copy, Clone)]
pub struct RealFileSystem;

impl FileSystem for RealFileSystem {
    fn absolute<P: AsRef<Path>>(&self, path: P) -> std::io::Result<PathBuf> {
        Ok(std::path::absolute(path)?.clean())
    }
}

/// Normalize a project path to the relative path from the workspace root.
/// In workspace root:
/// './project/' -> 'project'
/// 'project/sub/' -> 'project/sub'
/// '.' -> ''
/// './' -> ''
/// '..' -> Error
/// '../project/' -> Error
/// '/absolute/path/' -> if inside workspace, relative path; else Error
/// In subdirectory of workspace (e.g., 'workspace/subdir'):
/// './project/' -> 'subdir/project'
/// 'project/' -> 'subdir/project'
/// '../project/' -> 'project'
/// '..' -> ''
fn normalize_path<FS: FileSystem>(
    path: &str,
    workspace_root: &Path,
    fs: FS,
) -> Result<PathBuf, Error> {
    let absolute_path = fs.absolute(path).map_err(|e| Error::InvalidPath {
        path: path.to_string(),
        source: anyhow!(e),
    })?;
    let absolute_path = absolute_path.clean();

    let rel_path = absolute_path
        .strip_prefix(workspace_root)
        .map_err(|_| Error::InvalidPath {
            path: path.to_string(),
            source: anyhow!("path is outside the workspace root"),
        })?;

    Ok(rel_path.to_path_buf())
}

/// Parse skip selectors from CLI args and environment variables
pub(crate) fn load_skips<FS: FileSystem>(
    cli_skips: &[String],
    workspace_root: &Path,
    fs: FS,
) -> Result<Vec<Selector>, Error> {
    let prek_skip = EnvVars::var(EnvVars::PREK_SKIP);
    let skip = EnvVars::var(EnvVars::SKIP);

    let (skips, source) = if !cli_skips.is_empty() {
        (
            cli_skips.iter().map(String::as_str).collect::<Vec<_>>(),
            SelectorSource::CliFlag("--skip"),
        )
    } else if let Ok(s) = &prek_skip {
        (
            parse_comma_separated(s).collect(),
            SelectorSource::EnvVar(EnvVars::PREK_SKIP),
        )
    } else if let Ok(s) = &skip {
        (
            parse_comma_separated(s).collect(),
            SelectorSource::EnvVar(EnvVars::SKIP),
        )
    } else {
        return Ok(vec![]);
    };

    skips
        .into_iter()
        .unique()
        .map(|skip| parse_single_selector(skip, workspace_root, source, fs))
        .collect()
}

/// Parse comma-separated values, trimming whitespace and filtering empty strings
fn parse_comma_separated(input: &str) -> impl Iterator<Item = &str> {
    input.split(',').map(str::trim).filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    struct MockFileSystem {
        current_dir: TempDir,
    }

    impl FileSystem for &MockFileSystem {
        fn absolute<P: AsRef<Path>>(&self, path: P) -> std::io::Result<PathBuf> {
            let p = path.as_ref();
            if p.is_absolute() {
                Ok(p.to_path_buf())
            } else {
                Ok(self.current_dir.path().join(p))
            }
        }
    }

    impl MockFileSystem {
        fn root(&self) -> &Path {
            self.current_dir.path()
        }
    }

    fn create_test_workspace() -> anyhow::Result<MockFileSystem> {
        let temp_dir = TempDir::new()?;

        std::fs::create_dir_all(temp_dir.path().join("src"))?;
        std::fs::create_dir_all(temp_dir.path().join("src/backend"))?;

        Ok(MockFileSystem {
            current_dir: temp_dir,
        })
    }

    #[test]
    fn test_parse_single_selector_hook_id() -> anyhow::Result<()> {
        let fs = create_test_workspace()?;

        // Test explicit hook ID with colon prefix
        let selector = parse_single_selector(":black", fs.root(), SelectorSource::CliArg, &fs)?;
        assert!(matches!(selector.expr, SelectorExpr::HookId(ref id) if id == "black"));

        // Test bare hook ID (backward compatibility)
        let selector = parse_single_selector("black", fs.root(), SelectorSource::CliArg, &fs)?;
        assert!(matches!(selector.expr, SelectorExpr::HookId(ref id) if id == "black"));

        Ok(())
    }

    #[test]
    fn test_parse_single_selector_project_prefix() -> anyhow::Result<()> {
        let fs = create_test_workspace()?;

        // Test project path with slash
        let selector = parse_single_selector("src/", fs.root(), SelectorSource::CliArg, &fs)?;
        assert!(
            matches!(selector.expr, SelectorExpr::ProjectPrefix(ref path) if path == &PathBuf::from("src"))
        );

        // Test current directory
        let selector = parse_single_selector(".", fs.root(), SelectorSource::CliArg, &fs)?;
        assert!(
            matches!(selector.expr, SelectorExpr::ProjectPrefix(ref path) if path == &PathBuf::from(""))
        );
        let selector = parse_single_selector("./", fs.root(), SelectorSource::CliArg, &fs)?;
        assert!(
            matches!(selector.expr, SelectorExpr::ProjectPrefix(ref path) if path == &PathBuf::from(""))
        );

        Ok(())
    }

    #[test]
    fn test_parse_single_selector_project_hook() -> anyhow::Result<()> {
        let fs = create_test_workspace()?;

        let selector = parse_single_selector("src:black", fs.root(), SelectorSource::CliArg, &fs)?;
        match selector.expr {
            SelectorExpr::ProjectHook {
                project_path,
                hook_id,
            } => {
                assert_eq!(project_path, PathBuf::from("src"));
                assert_eq!(hook_id, "black");
            }
            _ => panic!("Expected ProjectHook"),
        }

        Ok(())
    }

    #[test]
    fn test_parse_single_selector_invalid() -> anyhow::Result<()> {
        let fs = create_test_workspace()?;

        // Test empty hook ID
        let result = parse_single_selector(":", fs.root(), SelectorSource::CliArg, &fs);
        assert!(result.is_err());

        // Test empty hook ID in project:hook
        let result = parse_single_selector("src:", fs.root(), SelectorSource::CliArg, &fs);
        assert!(result.is_err());

        // Test multiple colons
        let result =
            parse_single_selector("src:black:extra", fs.root(), SelectorSource::CliArg, &fs);
        assert!(result.is_err());

        // Test empty string
        let result = parse_single_selector("", fs.root(), SelectorSource::CliArg, &fs);
        assert!(result.is_err());

        Ok(())
    }

    #[test]
    fn test_normalize_path() -> anyhow::Result<()> {
        let fs = create_test_workspace()?;

        // Test relative path
        let result = normalize_path("src", fs.root(), &fs)?;
        assert_eq!(result, PathBuf::from("src"));

        // Test nested path
        let result = normalize_path("src/backend", fs.root(), &fs)?;
        assert_eq!(result, PathBuf::from("src/backend"));

        // Test current directory
        let result = normalize_path(".", fs.root(), &fs)?;
        assert_eq!(result, PathBuf::from(""));

        // Test path outside workspace - create a temp dir outside workspace
        let outside_dir = TempDir::new()?;
        let outside_path = outside_dir.path().to_string_lossy();
        let result = normalize_path(&outside_path, fs.root(), &fs);
        assert!(result.is_err());

        Ok(())
    }

    #[test]
    fn test_selector_display() -> anyhow::Result<()> {
        let fs = create_test_workspace()?;

        let selector = parse_single_selector("black", fs.root(), SelectorSource::CliArg, &fs)?;
        assert_eq!(selector.to_string(), "black");

        let selector = parse_single_selector(":black", fs.root(), SelectorSource::CliArg, &fs)?;
        assert_eq!(selector.to_string(), "black");

        let selector = parse_single_selector("src/", fs.root(), SelectorSource::CliArg, &fs)?;
        assert_eq!(selector.to_string(), "src/");

        let selector = parse_single_selector("./src/", fs.root(), SelectorSource::CliArg, &fs)?;
        assert_eq!(selector.to_string(), "src/");

        let selector = parse_single_selector("src/", fs.root(), SelectorSource::CliArg, &fs)?;
        assert_eq!(selector.to_string(), "src/");

        let selector = parse_single_selector(".", fs.root(), SelectorSource::CliArg, &fs)?;
        assert_eq!(selector.to_string(), "./");

        let selector = parse_single_selector("./", fs.root(), SelectorSource::CliArg, &fs)?;
        assert_eq!(selector.to_string(), "./");

        let selector = parse_single_selector("src:black", fs.root(), SelectorSource::CliArg, &fs)?;
        assert_eq!(selector.to_string(), "src:black");

        let selector =
            parse_single_selector("./src:black", fs.root(), SelectorSource::CliArg, &fs)?;
        assert_eq!(selector.to_string(), "src:black");

        let selector =
            parse_single_selector("./src/:black", fs.root(), SelectorSource::CliArg, &fs)?;
        assert_eq!(selector.to_string(), "src:black");

        Ok(())
    }

    #[test]
    fn test_selector_as_flag() {
        let selector = Selector {
            source: SelectorSource::CliArg,
            original: "black".to_string(),
            expr: SelectorExpr::HookId("black".to_string()),
        };
        assert_eq!(selector.as_flag(), "black");

        let selector = Selector {
            source: SelectorSource::CliFlag("--skip"),
            original: "black".to_string(),
            expr: SelectorExpr::HookId("black".to_string()),
        };
        assert_eq!(selector.as_flag(), "--skip=black");

        let selector = Selector {
            source: SelectorSource::EnvVar("SKIP"),
            original: "black".to_string(),
            expr: SelectorExpr::HookId("black".to_string()),
        };
        assert_eq!(selector.as_flag(), "SKIP=black");
    }
}
