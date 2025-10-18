use std::fmt::Write;
use std::path::Path;

use anyhow::Result;
use futures::StreamExt;
use ruff_python_ast::visitor::Visitor;
use ruff_python_ast::{Expr, Stmt};
use ruff_python_parser::parse_module;
use rustc_hash::FxHashSet;

use crate::hook::Hook;
use crate::run::CONCURRENCY;

const DEBUG_STATEMENTS: &[&str] = &[
    "bpdb",
    "ipdb",
    "pdb",
    "pdbr",
    "pudb",
    "pydevd_pycharm",
    "q",
    "rdb",
    "rpdb",
    "wdb",
];

#[derive(Debug)]
struct DebugInfo {
    line: usize,
    col: usize,
    name: String,
    reason: String,
}

struct DebugStatementVisitor<'a> {
    breakpoints: Vec<DebugInfo>,
    debug_set: FxHashSet<String>,
    source: &'a str,
}

impl<'a> DebugStatementVisitor<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            breakpoints: Vec::new(),
            debug_set: DEBUG_STATEMENTS.iter().map(|s| (*s).to_string()).collect(),
            source,
        }
    }

    fn get_line_col(&self, offset: usize) -> (usize, usize) {
        let mut line = 1;
        let mut col = 0;

        for (i, ch) in self.source.bytes().enumerate() {
            if i == offset {
                break;
            }
            if ch == b'\n' {
                line += 1;
                col = 0;
            } else {
                col += 1;
            }
        }

        (line, col)
    }
}

impl Visitor<'_> for DebugStatementVisitor<'_> {
    fn visit_stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Import(import) => {
                for alias in &import.names {
                    if self.debug_set.contains(alias.name.as_str()) {
                        let (line, col) = self.get_line_col(import.range.start().to_usize());
                        self.breakpoints.push(DebugInfo {
                            line,
                            col,
                            name: alias.name.to_string(),
                            reason: "imported".to_string(),
                        });
                    }
                }
            }
            Stmt::ImportFrom(import_from) => {
                if let Some(module) = &import_from.module {
                    if self.debug_set.contains(module.as_str()) {
                        let (line, col) = self.get_line_col(import_from.range.start().to_usize());
                        self.breakpoints.push(DebugInfo {
                            line,
                            col,
                            name: module.to_string(),
                            reason: "imported".to_string(),
                        });
                    }
                }
            }
            _ => {}
        }
        ruff_python_ast::visitor::walk_stmt(self, stmt);
    }

    fn visit_expr(&mut self, expr: &Expr) {
        if let Expr::Call(call) = expr {
            if let Expr::Name(name) = call.func.as_ref() {
                if name.id.as_str() == "breakpoint" {
                    let (line, col) = self.get_line_col(call.range.start().to_usize());
                    self.breakpoints.push(DebugInfo {
                        line,
                        col,
                        name: "breakpoint".to_string(),
                        reason: "called".to_string(),
                    });
                }
            }
        }
        ruff_python_ast::visitor::walk_expr(self, expr);
    }
}

pub(crate) async fn debug_statements(hook: &Hook, filenames: &[&Path]) -> Result<(i32, Vec<u8>)> {
    let mut tasks = futures::stream::iter(filenames)
        .map(async |filename| check_file(hook.project().relative_path(), filename).await)
        .buffered(*CONCURRENCY);

    let mut code = 0;
    let mut output = Vec::new();

    while let Some(result) = tasks.next().await {
        let (c, o) = result?;
        code |= c;
        output.extend(o);
    }

    Ok((code, output))
}

async fn check_file(file_base: &Path, filename: &Path) -> Result<(i32, Vec<u8>)> {
    let content = fs_err::tokio::read_to_string(file_base.join(filename)).await?;

    let parse_result = parse_module(&content);

    let module = match parse_result {
        Ok(parsed) => {
            if !parsed.errors().is_empty() {
                let mut error_output = format!("{} - Could not parse ast\n\n", filename.display());
                for error in parsed.errors() {
                    let _ = writeln!(error_output, "\t{error}");
                }
                error_output.push('\n');
                return Ok((1, error_output.into_bytes()));
            }
            parsed.into_syntax()
        }
        Err(e) => {
            let error_message = format!(
                "{} - Could not parse ast\n\n\t{}\n\n",
                filename.display(),
                e
            );
            return Ok((1, error_message.into_bytes()));
        }
    };

    let mut visitor = DebugStatementVisitor::new(&content);
    visitor.visit_body(&module.body);

    let mut output = Vec::new();
    for bp in &visitor.breakpoints {
        let line = format!(
            "{}:{}:{}: {} {}\n",
            filename.display(),
            bp.line,
            bp.col,
            bp.name,
            bp.reason
        );
        output.extend(line.into_bytes());
    }

    let code = i32::from(!visitor.breakpoints.is_empty());

    Ok((code, output))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::tempdir;

    async fn create_test_file(
        dir: &tempfile::TempDir,
        name: &str,
        content: &str,
    ) -> Result<PathBuf> {
        let file_path = dir.path().join(name);
        fs_err::tokio::write(&file_path, content).await?;
        Ok(file_path)
    }

    #[tokio::test]
    async fn test_no_debug_statements() -> Result<()> {
        let dir = tempdir()?;
        let content = r#"
def hello():
    print("Hello, world!")
"#;
        let file_path = create_test_file(&dir, "clean.py", content).await?;
        let (code, output) = check_file(Path::new(""), &file_path).await?;
        assert_eq!(code, 0);
        assert!(output.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn test_import_pdb() -> Result<()> {
        let dir = tempdir()?;
        let content = r"
import pdb

def debug_func():
    x = 1
";
        let file_path = create_test_file(&dir, "debug.py", content).await?;
        let (code, output) = check_file(Path::new(""), &file_path).await?;
        assert_eq!(code, 1);
        let output_str = String::from_utf8_lossy(&output);
        assert!(output_str.contains("pdb imported"));
        assert!(output_str.contains("debug.py:2:"));
        Ok(())
    }

    #[tokio::test]
    async fn test_import_ipdb() -> Result<()> {
        let dir = tempdir()?;
        let content = r"
import ipdb
import sys
";
        let file_path = create_test_file(&dir, "debug.py", content).await?;
        let (code, output) = check_file(Path::new(""), &file_path).await?;
        assert_eq!(code, 1);
        let output_str = String::from_utf8_lossy(&output);
        assert!(output_str.contains("ipdb imported"));
        Ok(())
    }

    #[tokio::test]
    async fn test_from_import() -> Result<()> {
        let dir = tempdir()?;
        let content = r"
from pdb import set_trace
";
        let file_path = create_test_file(&dir, "debug.py", content).await?;
        let (code, output) = check_file(Path::new(""), &file_path).await?;
        assert_eq!(code, 1);
        let output_str = String::from_utf8_lossy(&output);
        assert!(output_str.contains("pdb imported"));
        Ok(())
    }

    #[tokio::test]
    async fn test_breakpoint_call() -> Result<()> {
        let dir = tempdir()?;
        let content = r"
def debug_func():
    x = 1
    breakpoint()
    return x
";
        let file_path = create_test_file(&dir, "debug.py", content).await?;
        let (code, output) = check_file(Path::new(""), &file_path).await?;
        assert_eq!(code, 1);
        let output_str = String::from_utf8_lossy(&output);
        assert!(output_str.contains("breakpoint called"));
        assert!(output_str.contains("debug.py:4:"));
        Ok(())
    }

    #[tokio::test]
    async fn test_multiple_debug_statements() -> Result<()> {
        let dir = tempdir()?;
        let content = r"
import pdb
import ipdb

def debug_func():
    breakpoint()
    x = 1
";
        let file_path = create_test_file(&dir, "debug.py", content).await?;
        let (code, output) = check_file(Path::new(""), &file_path).await?;
        assert_eq!(code, 1);
        let output_str = String::from_utf8_lossy(&output);
        assert!(output_str.contains("pdb imported"));
        assert!(output_str.contains("ipdb imported"));
        assert!(output_str.contains("breakpoint called"));
        Ok(())
    }

    #[tokio::test]
    async fn test_pudb_import() -> Result<()> {
        let dir = tempdir()?;
        let content = r"
import pudb
";
        let file_path = create_test_file(&dir, "debug.py", content).await?;
        let (code, output) = check_file(Path::new(""), &file_path).await?;
        assert_eq!(code, 1);
        let output_str = String::from_utf8_lossy(&output);
        assert!(output_str.contains("pudb imported"));
        Ok(())
    }

    #[tokio::test]
    async fn test_syntax_error() -> Result<()> {
        let dir = tempdir()?;
        let content = r#"
def broken(
    print("syntax error")
"#;
        let file_path = create_test_file(&dir, "broken.py", content).await?;
        let (code, output) = check_file(Path::new(""), &file_path).await?;
        assert_eq!(code, 1);
        let output_str = String::from_utf8_lossy(&output);
        assert!(output_str.contains("Could not parse ast"));
        Ok(())
    }

    #[tokio::test]
    async fn test_multiple_imports_same_line() -> Result<()> {
        let dir = tempdir()?;
        let content = r"
import pdb, ipdb, sys
";
        let file_path = create_test_file(&dir, "debug.py", content).await?;
        let (code, output) = check_file(Path::new(""), &file_path).await?;
        assert_eq!(code, 1);
        let output_str = String::from_utf8_lossy(&output);
        assert!(output_str.contains("pdb imported"));
        assert!(output_str.contains("ipdb imported"));
        // Should not contain sys
        assert!(!output_str.contains("sys imported"));
        Ok(())
    }

    #[tokio::test]
    async fn test_breakpoint_in_expression() -> Result<()> {
        let dir = tempdir()?;
        let content = r#"
result = breakpoint() or "default"
"#;
        let file_path = create_test_file(&dir, "debug.py", content).await?;
        let (code, output) = check_file(Path::new(""), &file_path).await?;
        assert_eq!(code, 1);
        let output_str = String::from_utf8_lossy(&output);
        assert!(output_str.contains("breakpoint called"));
        Ok(())
    }

    #[tokio::test]
    async fn test_no_false_positive_breakpoint_string() -> Result<()> {
        let dir = tempdir()?;
        let content = r#"
# This should not trigger
message = "breakpoint"
def breakpoint_func():
    pass
"#;
        let file_path = create_test_file(&dir, "clean.py", content).await?;
        let (code, output) = check_file(Path::new(""), &file_path).await?;
        assert_eq!(code, 0);
        assert!(output.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn test_all_debug_modules() -> Result<()> {
        let dir = tempdir()?;
        let content = r"
import bpdb
import ipdb
import pdb
import pdbr
import pudb
import pydevd_pycharm
import q
import rdb
import rpdb
import wdb
";
        let file_path = create_test_file(&dir, "all_debug.py", content).await?;
        let (code, output) = check_file(Path::new(""), &file_path).await?;
        assert_eq!(code, 1);
        let output_str = String::from_utf8_lossy(&output);
        // Check that all debug modules are detected
        assert!(output_str.contains("bpdb imported"));
        assert!(output_str.contains("ipdb imported"));
        assert!(output_str.contains("pdb imported"));
        assert!(output_str.contains("pdbr imported"));
        assert!(output_str.contains("pudb imported"));
        assert!(output_str.contains("pydevd_pycharm imported"));
        assert!(output_str.contains("q imported"));
        assert!(output_str.contains("rdb imported"));
        assert!(output_str.contains("rpdb imported"));
        assert!(output_str.contains("wdb imported"));
        Ok(())
    }
}
