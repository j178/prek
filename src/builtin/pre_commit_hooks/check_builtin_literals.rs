use std::fmt::Write;
use std::path::Path;

use anyhow::Result;
use clap::Parser;
use futures::StreamExt;
use ruff_python_ast::Expr;
use ruff_python_ast::visitor::Visitor;
use ruff_python_parser::parse_module;
use rustc_hash::FxHashSet;

use crate::hook::Hook;
use crate::run::CONCURRENCY;

const BUILTIN_TYPES: &[(&str, &str)] = &[
    ("complex", "0j"),
    ("dict", "{}"),
    ("float", "0.0"),
    ("int", "0"),
    ("list", "[]"),
    ("str", "''"),
    ("tuple", "()"),
];

#[derive(Parser)]
#[command(disable_help_subcommand = true)]
#[command(disable_version_flag = true)]
#[command(disable_help_flag = true)]
struct Args {
    #[arg(long, value_delimiter = ',')]
    ignore: Vec<String>,
    #[arg(long, default_value = "true", action = clap::ArgAction::SetTrue)]
    allow_dict_kwargs: bool,
    #[arg(long = "no-allow-dict-kwargs", action = clap::ArgAction::SetFalse)]
    no_allow_dict_kwargs: bool,
}

#[derive(Debug)]
struct Call {
    name: String,
    line: usize,
    col: usize,
}

struct BuiltinLiteralsVisitor<'a> {
    calls: Vec<Call>,
    ignore: FxHashSet<String>,
    allow_dict_kwargs: bool,
    builtin_types: FxHashSet<String>,
    source: &'a str,
}

impl<'a> BuiltinLiteralsVisitor<'a> {
    fn new(source: &'a str, ignore: FxHashSet<String>, allow_dict_kwargs: bool) -> Self {
        let builtin_types = BUILTIN_TYPES
            .iter()
            .map(|(name, _)| (*name).to_string())
            .collect();

        Self {
            calls: Vec::new(),
            ignore,
            allow_dict_kwargs,
            builtin_types,
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

impl Visitor<'_> for BuiltinLiteralsVisitor<'_> {
    fn visit_expr(&mut self, expr: &Expr) {
        if let Expr::Call(call) = expr {
            // Only check if func is a simple Name (not an attribute like foo.bar())
            if let Expr::Name(name) = call.func.as_ref() {
                let func_name = name.id.as_str();

                // Check if it's a builtin type we care about
                if self.builtin_types.contains(func_name) && !self.ignore.contains(func_name) {
                    // Special handling for dict() with kwargs
                    if func_name == "dict"
                        && self.allow_dict_kwargs
                        && !call.arguments.keywords.is_empty()
                    {
                        // Allow dict(foo=bar)
                    } else if call.arguments.args.is_empty() && call.arguments.keywords.is_empty() {
                        // Empty call like list() or dict()
                        let (line, col) = self.get_line_col(call.range.start().to_usize());
                        self.calls.push(Call {
                            name: func_name.to_string(),
                            line,
                            col,
                        });
                    }
                }
            }
        }
        ruff_python_ast::visitor::walk_expr(self, expr);
    }
}

pub(crate) async fn check_builtin_literals(
    hook: &Hook,
    filenames: &[&Path],
) -> Result<(i32, Vec<u8>)> {
    let args = Args::try_parse_from(hook.entry.resolve(None)?.iter().chain(&hook.args))?;

    let ignore: FxHashSet<String> = args.ignore.into_iter().collect();
    let allow_dict_kwargs = if args.no_allow_dict_kwargs {
        false
    } else {
        args.allow_dict_kwargs
    };

    let mut tasks = futures::stream::iter(filenames)
        .map(|filename| {
            let ignore = ignore.clone();
            async move {
                check_file(
                    hook.project().relative_path(),
                    filename,
                    &ignore,
                    allow_dict_kwargs,
                )
                .await
            }
        })
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

async fn check_file(
    file_base: &Path,
    filename: &Path,
    ignore: &FxHashSet<String>,
    allow_dict_kwargs: bool,
) -> Result<(i32, Vec<u8>)> {
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

    let mut visitor = BuiltinLiteralsVisitor::new(&content, ignore.clone(), allow_dict_kwargs);
    visitor.visit_body(&module.body);

    let mut output = Vec::new();
    for call in &visitor.calls {
        let replacement = BUILTIN_TYPES
            .iter()
            .find(|(name, _)| *name == call.name)
            .map(|(_, repl)| *repl)
            .unwrap_or("");

        let line = format!(
            "{}:{}:{}: replace {}() with {}\n",
            filename.display(),
            call.line,
            call.col,
            call.name,
            replacement
        );
        output.extend(line.into_bytes());
    }

    let code = i32::from(!visitor.calls.is_empty());

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
    async fn test_no_builtin_calls() -> Result<()> {
        let dir = tempdir()?;
        let content = r"
x = [1, 2, 3]
y = {'key': 'value'}
z = 'string'
";
        let file_path = create_test_file(&dir, "clean.py", content).await?;
        let ignore = FxHashSet::default();
        let (code, output) = check_file(Path::new(""), &file_path, &ignore, true).await?;
        assert_eq!(code, 0);
        assert!(output.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn test_empty_list_call() -> Result<()> {
        let dir = tempdir()?;
        let content = r"
x = list()
";
        let file_path = create_test_file(&dir, "test.py", content).await?;
        let ignore = FxHashSet::default();
        let (code, output) = check_file(Path::new(""), &file_path, &ignore, true).await?;
        assert_eq!(code, 1);
        let output_str = String::from_utf8_lossy(&output);
        assert!(output_str.contains("replace list() with []"));
        Ok(())
    }

    #[tokio::test]
    async fn test_empty_dict_call() -> Result<()> {
        let dir = tempdir()?;
        let content = r"
x = dict()
";
        let file_path = create_test_file(&dir, "test.py", content).await?;
        let ignore = FxHashSet::default();
        let (code, output) = check_file(Path::new(""), &file_path, &ignore, true).await?;
        assert_eq!(code, 1);
        let output_str = String::from_utf8_lossy(&output);
        assert!(output_str.contains("replace dict() with {}"));
        Ok(())
    }

    #[tokio::test]
    async fn test_dict_with_kwargs_allowed() -> Result<()> {
        let dir = tempdir()?;
        let content = r"
x = dict(foo='bar')
";
        let file_path = create_test_file(&dir, "test.py", content).await?;
        let ignore = FxHashSet::default();
        let (code, output) = check_file(Path::new(""), &file_path, &ignore, true).await?;
        assert_eq!(code, 0);
        assert!(output.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn test_dict_with_kwargs_not_allowed() -> Result<()> {
        let dir = tempdir()?;
        let content = r"
x = dict(foo='bar')
";
        let file_path = create_test_file(&dir, "test.py", content).await?;
        let ignore = FxHashSet::default();
        let (code, output) = check_file(Path::new(""), &file_path, &ignore, false).await?;
        assert_eq!(code, 0); // Should still be 0 because it has arguments
        assert!(output.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn test_list_with_args() -> Result<()> {
        let dir = tempdir()?;
        let content = r"
x = list([1, 2, 3])
";
        let file_path = create_test_file(&dir, "test.py", content).await?;
        let ignore = FxHashSet::default();
        let (code, output) = check_file(Path::new(""), &file_path, &ignore, true).await?;
        assert_eq!(code, 0);
        assert!(output.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn test_multiple_builtin_calls() -> Result<()> {
        let dir = tempdir()?;
        let content = r"
x = list()
y = dict()
z = tuple()
";
        let file_path = create_test_file(&dir, "test.py", content).await?;
        let ignore = FxHashSet::default();
        let (code, output) = check_file(Path::new(""), &file_path, &ignore, true).await?;
        assert_eq!(code, 1);
        let output_str = String::from_utf8_lossy(&output);
        assert!(output_str.contains("replace list() with []"));
        assert!(output_str.contains("replace dict() with {}"));
        assert!(output_str.contains("replace tuple() with ()"));
        Ok(())
    }

    #[tokio::test]
    async fn test_ignore_specific_type() -> Result<()> {
        let dir = tempdir()?;
        let content = r"
x = list()
y = dict()
";
        let file_path = create_test_file(&dir, "test.py", content).await?;
        let mut ignore = FxHashSet::default();
        ignore.insert("list".to_string());
        let (code, output) = check_file(Path::new(""), &file_path, &ignore, true).await?;
        assert_eq!(code, 1);
        let output_str = String::from_utf8_lossy(&output);
        assert!(!output_str.contains("list"));
        assert!(output_str.contains("replace dict() with {}"));
        Ok(())
    }

    #[tokio::test]
    async fn test_attribute_call_ignored() -> Result<()> {
        let dir = tempdir()?;
        let content = r"
x = builtins.list()
y = foo.dict()
";
        let file_path = create_test_file(&dir, "test.py", content).await?;
        let ignore = FxHashSet::default();
        let (code, output) = check_file(Path::new(""), &file_path, &ignore, true).await?;
        assert_eq!(code, 0);
        assert!(output.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn test_all_builtin_types() -> Result<()> {
        let dir = tempdir()?;
        let content = r"
a = complex()
b = dict()
c = float()
d = int()
e = list()
f = str()
g = tuple()
";
        let file_path = create_test_file(&dir, "test.py", content).await?;
        let ignore = FxHashSet::default();
        let (code, output) = check_file(Path::new(""), &file_path, &ignore, true).await?;
        assert_eq!(code, 1);
        let output_str = String::from_utf8_lossy(&output);
        assert!(output_str.contains("replace complex() with 0j"));
        assert!(output_str.contains("replace dict() with {}"));
        assert!(output_str.contains("replace float() with 0.0"));
        assert!(output_str.contains("replace int() with 0"));
        assert!(output_str.contains("replace list() with []"));
        assert!(output_str.contains("replace str() with ''"));
        assert!(output_str.contains("replace tuple() with ()"));
        Ok(())
    }
}
