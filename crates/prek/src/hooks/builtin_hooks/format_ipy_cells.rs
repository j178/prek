use std::path::Path;

use anyhow::Result;

use crate::hook::Hook;
use crate::hooks::run_concurrent_file_checks;
use crate::run::CONCURRENCY;

/// A single cell in an interactive Python notebook, delimited by `# %%`.
struct Cell {
    /// Optional comment text after `# %%` on the delimiter line.
    comment: Option<String>,
    /// Content lines between this delimiter and the next (or end of file).
    lines: Vec<String>,
}

/// Parsed representation of an interactive Python notebook.
struct Notebook {
    /// Lines before the first `# %%` delimiter (e.g. module docstring, imports).
    preamble: Vec<String>,
    /// Sequence of cells, each starting with a `# %%` delimiter.
    cells: Vec<Cell>,
}

pub(crate) async fn format_ipy_cells(hook: &Hook, filenames: &[&Path]) -> Result<(i32, Vec<u8>)> {
    run_concurrent_file_checks(filenames.iter().copied(), *CONCURRENCY, |filename| {
        fix_file(hook.project().relative_path(), filename)
    })
    .await
}

async fn fix_file(file_base: &Path, filename: &Path) -> Result<(i32, Vec<u8>)> {
    let file_path = file_base.join(filename);
    let original = fs_err::tokio::read_to_string(&file_path).await?;

    let formatted = format_text(&original);
    if formatted == original {
        return Ok((0, Vec::new()));
    }

    fs_err::tokio::write(&file_path, &formatted).await?;
    Ok((1, format!("Fixing {}\n", filename.display()).into_bytes()))
}

/// Try to parse a line as a cell delimiter (`# %%`).
///
/// Returns `Some(Some(comment))` if the line is a delimiter with a comment,
/// `Some(None)` if it is a bare delimiter, or `None` if the line is not a
/// delimiter at all.
///
/// The `#` must appear at column 0 (no leading whitespace), matching the
/// original `^#\s*%%` regex behavior. This avoids false positives on
/// indented comments like `    # %% section` inside control flow.
fn parse_delimiter(line: &str) -> Option<Option<String>> {
    let rest = line.strip_prefix('#')?;
    let rest = rest.trim_start();
    let rest = rest.strip_prefix("%%")?;

    let comment_text = rest.trim();
    if comment_text.is_empty() {
        Some(None)
    } else {
        Some(Some(comment_text.to_string()))
    }
}

/// Parse file contents into a structured `Notebook`.
fn parse(text: &str) -> Notebook {
    let mut preamble: Vec<String> = Vec::new();
    let mut cells: Vec<Cell> = Vec::new();

    for raw_line in text.lines() {
        let line = raw_line.trim_end().to_string();

        if let Some(comment) = parse_delimiter(&line) {
            cells.push(Cell {
                comment,
                lines: Vec::new(),
            });
        } else if let Some(cell) = cells.last_mut() {
            cell.lines.push(line);
        } else {
            preamble.push(line);
        }
    }

    Notebook { preamble, cells }
}

/// Remove leading and trailing blank lines from a list of lines.
fn trim_blank_lines(lines: &mut Vec<String>) {
    while lines.first().is_some_and(|line| line.trim().is_empty()) {
        lines.remove(0);
    }
    while lines.last().is_some_and(|line| line.trim().is_empty()) {
        lines.pop();
    }
}

/// Apply formatting rules to the notebook in place.
fn format_notebook(nb: &mut Notebook) {
    trim_blank_lines(&mut nb.preamble);

    // Merge bare delimiters: when a bare `# %%` (no comment) follows another
    // cell with only whitespace content between them, remove the bare delimiter
    // and merge its content into the previous cell. This matches the behavior
    // of the original Python regex `^# %%([^\n]*)(?:\s+# %%$)+`.
    let mut merged: Vec<Cell> = Vec::new();
    for cell in nb.cells.drain(..) {
        if cell.comment.is_none() {
            if let Some(prev) = merged.last_mut() {
                if prev.lines.iter().all(|line| line.trim().is_empty()) {
                    prev.lines.extend(cell.lines);
                    continue;
                }
            }
        }
        merged.push(cell);
    }
    nb.cells = merged;

    // Remove truly empty cells (no comment AND no non-blank content)
    nb.cells.retain(|cell| {
        cell.comment.is_some() || cell.lines.iter().any(|line| !line.trim().is_empty())
    });

    for cell in &mut nb.cells {
        trim_blank_lines(&mut cell.lines);
    }
}

/// Check whether `line` ends with a triple-quote (`"""` or `'''`).
fn ends_with_triple_quote(line: &str) -> bool {
    line.ends_with("\"\"\"") || line.ends_with("'''")
}

/// Serialize a formatted `Notebook` back into text.
fn serialize(nb: &Notebook) -> String {
    let mut output = String::new();

    // Emit preamble
    for line in &nb.preamble {
        output.push_str(line);
        output.push('\n');
    }

    for (cell_idx, cell) in nb.cells.iter().enumerate() {
        // Determine what immediately precedes this cell for spacing decisions.
        let preceding_last_line = if cell_idx == 0 {
            nb.preamble.last().map(|s| s.as_str())
        } else {
            let prev = &nb.cells[cell_idx - 1];
            prev.lines.last().map(|s| s.as_str())
        };

        // Spacing before this cell's delimiter
        if cell_idx == 0 && nb.preamble.is_empty() {
            // First cell, no preamble — no spacing needed
        } else if preceding_last_line.is_some_and(|line| ends_with_triple_quote(line)) {
            // After a triple-quoted string: one blank line (ruff compatibility)
            output.push('\n');
        } else {
            // Standard spacing: two blank lines before cell delimiter
            output.push_str("\n\n");
        }

        // Emit the delimiter line
        output.push_str("# %%");
        if let Some(comment) = &cell.comment {
            output.push(' ');
            output.push_str(comment);
        }
        output.push('\n');

        // Emit cell content
        for line in &cell.lines {
            output.push_str(line);
            output.push('\n');
        }
    }

    output
}

/// Format interactive Python notebook text.
///
/// Parses the text into a structured notebook, applies formatting rules,
/// and serializes back to a string.
fn format_text(text: &str) -> String {
    let mut nb = parse(text);

    // If there are no cells, return the original text unchanged
    if nb.cells.is_empty() {
        return text.to_string();
    }

    format_notebook(&mut nb);
    serialize(&nb)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::PathBuf;
    use tempfile::TempDir;

    async fn create_test_file(dir: &TempDir, name: &str, content: &str) -> Result<PathBuf> {
        let file_path = dir.path().join(name);
        fs_err::tokio::write(&file_path, content).await?;
        Ok(file_path)
    }

    // === Parsing ===

    #[test]
    fn test_parse_delimiter_variants() {
        // Standard delimiter
        assert_eq!(parse_delimiter("# %%"), Some(None));
        // No space
        assert_eq!(parse_delimiter("#%%"), Some(None));
        // Extra spaces
        assert_eq!(parse_delimiter("#   %%"), Some(None));
        // With comment
        assert_eq!(
            parse_delimiter("# %% some comment"),
            Some(Some("some comment".to_string()))
        );
        // Comment with no space
        assert_eq!(
            parse_delimiter("# %%comment"),
            Some(Some("comment".to_string()))
        );
        // Comment with excessive space
        assert_eq!(
            parse_delimiter("# %%     comment"),
            Some(Some("comment".to_string()))
        );
        // Not a delimiter
        assert_eq!(parse_delimiter("some_code = 42"), None);
        assert_eq!(parse_delimiter("# regular comment"), None);
        assert_eq!(parse_delimiter(""), None);
        // Indented `# %%` is NOT a cell delimiter (must start at column 0)
        assert_eq!(parse_delimiter("    # %%"), None);
        assert_eq!(parse_delimiter("  # %% indented comment"), None);
    }

    #[test]
    fn test_parse_basic() {
        let input = "\
\"\"\"module doc.\"\"\"

# %%
foo = 1

# %% second cell
bar = 2
";
        let nb = parse(input);
        assert_eq!(nb.preamble.len(), 2);
        assert_eq!(nb.preamble[0], "\"\"\"module doc.\"\"\"");
        assert_eq!(nb.preamble[1], "");
        assert_eq!(nb.cells.len(), 2);
        assert!(nb.cells[0].comment.is_none());
        assert_eq!(nb.cells[0].lines, vec!["foo = 1", ""]);
        assert_eq!(nb.cells[1].comment, Some("second cell".to_string()));
        assert_eq!(nb.cells[1].lines, vec!["bar = 2"]);
    }

    // === Individual formatting rules ===

    #[test]
    fn test_normalize_delimiter_spacing() {
        let input = "#%%\nfoo = 1\n\n\n#   %%\nbar = 2\n";
        let result = format_text(input);
        assert!(result.contains("# %%\nfoo = 1"));
        assert!(result.contains("# %%\nbar = 2"));
        // No raw #%% or #   %%
        assert!(!result.contains("#%%"));
        assert!(!result.contains("#   %%"));
    }

    #[test]
    fn test_normalize_comment_spacing() {
        let input = "# %%comment\nfoo = 1\n\n\n# %%     another comment\nbar = 2\n";
        let result = format_text(input);
        assert!(result.contains("# %% comment\n"));
        assert!(result.contains("# %% another comment\n"));
    }

    #[test]
    fn test_remove_empty_cells() {
        let input = "# %% first\ncode = 1\n\n\n# %%\n\n\n# %% third\ncode = 3\n";
        let result = format_text(input);
        // The bare middle cell should be removed
        assert!(!result.contains("# %%\n\n\n# %%"));
        assert!(result.contains("# %% first\ncode = 1"));
        assert!(result.contains("# %% third\ncode = 3"));
    }

    #[test]
    fn test_keep_commented_empty_cells() {
        let input = "# %% keep this even though empty\n\n\n# %% has code\nfoo = 1\n";
        let result = format_text(input);
        assert!(result.contains("# %% keep this even though empty"));
    }

    #[test]
    fn test_bare_delimiter_merges_content_into_previous_cell() {
        // A bare `# %%` separated from the previous cell by only whitespace
        // should be removed, with its content flowing into the previous cell.
        let input = "# %% commented\n\n# %%\ncode_here = 1\n";
        let result = format_text(input);
        assert!(
            result.contains("# %% commented\ncode_here = 1"),
            "Content under bare delimiter should merge into previous cell, got: {result:?}"
        );
        assert_eq!(
            result.matches("# %%").count(),
            1,
            "Bare delimiter should be removed"
        );
    }

    #[test]
    fn test_bare_delimiter_kept_when_previous_has_code() {
        // A bare `# %%` after a cell with real code should be kept.
        let input = "# %% first\ncode = 1\n\n\n# %%\nmore_code = 2\n";
        let result = format_text(input);
        assert_eq!(
            result.matches("# %%").count(),
            2,
            "Bare cell should be kept when previous cell has content"
        );
        assert!(result.contains("# %%\nmore_code = 2"));
    }

    #[test]
    fn test_remove_trailing_empty_cell() {
        let input = "# %%\nsome_code = 1\n\n\n# %%\n\n";
        let result = format_text(input);
        // Trailing empty cell should be gone
        assert!(result.trim_end().ends_with("some_code = 1"));
    }

    #[test]
    fn test_blank_lines_between_cells() {
        // Too few blank lines between cells
        let input = "# %%\nfoo = 1\n# %%\nbar = 2\n";
        let result = format_text(input);
        assert!(result.contains("foo = 1\n\n\n# %%\nbar"));

        // Too many blank lines between cells
        let input2 = "# %%\nfoo = 1\n\n\n\n\n\n# %%\nbar = 2\n";
        let result2 = format_text(input2);
        assert!(result2.contains("foo = 1\n\n\n# %%\nbar"));
    }

    #[test]
    fn test_no_leading_blanks_in_cell() {
        let input = "# %% cell\n\n\n\nfirst_code = 'here'\n";
        let result = format_text(input);
        assert!(result.contains("# %% cell\nfirst_code = 'here'"));
    }

    #[test]
    fn test_docstring_spacing() {
        // Multiple blank lines between docstring and first cell -> exactly one
        let input = "\"\"\"module doc.\"\"\"\n\n\n\n# %%\ncode = 1\n";
        let result = format_text(input);
        assert_eq!(result, "\"\"\"module doc.\"\"\"\n\n# %%\ncode = 1\n");
    }

    #[test]
    fn test_docstring_single_quotes() {
        let input = "'''module doc.'''\n\n\n\n# %%\ncode = 1\n";
        let result = format_text(input);
        assert_eq!(result, "'''module doc.'''\n\n# %%\ncode = 1\n");
    }

    #[test]
    fn test_preamble_non_docstring_spacing() {
        // Preamble that doesn't end with a docstring gets two blank lines
        let input = "import os\n\n# %%\ncode = 1\n";
        let result = format_text(input);
        assert_eq!(result, "import os\n\n\n# %%\ncode = 1\n");
    }

    // === Meta properties ===

    #[test]
    fn test_idempotent() {
        let clean = "\
\"\"\"module doc string.\"\"\"

# %%
foo = \"hello\"


# %% a comment following a cell delimiter without space
bar = \"world\"


# %% a comment following a cell delimiter with too much space
baz = 42

\"\"\"longer comment\"\"\"

# %% empty cell with comment (should not be removed despite empty)


# %% a commented cell with white space on same line
bang = \"!\"
";
        let result = format_text(clean);
        assert_eq!(result, clean);

        // Second pass should also be identical
        let result2 = format_text(&result);
        assert_eq!(result2, clean);
    }

    #[test]
    fn test_no_cells() {
        let input = "import os\nimport sys\n\nfoo = 42\n";
        let result = format_text(input);
        assert_eq!(result, input, "File with no cells should be unchanged");
    }

    #[test]
    fn test_empty_file() {
        let result = format_text("");
        assert_eq!(result, "");
    }

    // === Full fixture test (raw_nb.py -> clean_nb.py) ===

    #[test]
    fn test_full_fixture() {
        let raw = "\
\"\"\"module doc string.\"\"\"


#  %%
foo = \"hello\"
#  %%a comment following a cell delimiter without space
bar = \"world\"
#  %%       a comment following a cell delimiter with too much space
baz = 42

\"\"\"longer comment\"\"\"

  \n \n\
#  %%    empty cell with comment (should not be removed despite empty)


# %%


#  %% a commented cell with white space on same line

#  %%


bang = \"!\"

#   %%
";
        let expected = "\
\"\"\"module doc string.\"\"\"

# %%
foo = \"hello\"


# %% a comment following a cell delimiter without space
bar = \"world\"


# %% a comment following a cell delimiter with too much space
baz = 42

\"\"\"longer comment\"\"\"

# %% empty cell with comment (should not be removed despite empty)


# %% a commented cell with white space on same line
bang = \"!\"
";
        let result = format_text(raw);
        assert_eq!(result, expected);
    }

    // === Async file tests ===

    #[tokio::test]
    async fn test_fix_file_modifies_dirty() -> Result<()> {
        let dir = TempDir::new()?;
        let content = "#%%\nfoo = 1\n#%%\nbar = 2\n";
        let file_path = create_test_file(&dir, "dirty.py", content).await?;

        let (code, output) = fix_file(Path::new(""), &file_path).await?;
        assert_eq!(code, 1);
        assert!(!output.is_empty());

        let new_content = fs_err::tokio::read_to_string(&file_path).await?;
        assert!(new_content.contains("# %%\nfoo = 1"));
        assert!(new_content.contains("# %%\nbar = 2"));

        Ok(())
    }

    #[tokio::test]
    async fn test_fix_file_no_change_for_clean() -> Result<()> {
        let dir = TempDir::new()?;
        let content = "# %%\nfoo = 1\n\n\n# %%\nbar = 2\n";
        let file_path = create_test_file(&dir, "clean.py", content).await?;

        let (code, output) = fix_file(Path::new(""), &file_path).await?;
        assert_eq!(code, 0);
        assert!(output.is_empty());

        Ok(())
    }

    #[tokio::test]
    async fn test_fix_file_no_cells_unchanged() -> Result<()> {
        let dir = TempDir::new()?;
        let content = "import os\nfoo = 42\n";
        let file_path = create_test_file(&dir, "nocells.py", content).await?;

        let (code, output) = fix_file(Path::new(""), &file_path).await?;
        assert_eq!(code, 0);
        assert!(output.is_empty());

        let new_content = fs_err::tokio::read_to_string(&file_path).await?;
        assert_eq!(new_content, content);

        Ok(())
    }
}
