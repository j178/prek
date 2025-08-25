// MIT License
//
// Copyright (c) 2025 Astral Software Inc.
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.

use std::path::Path;
use std::str::FromStr;
use std::sync::LazyLock;

use anyhow::Result;
use memchr::memmem::Finder;
use serde::Deserialize;
use tracing::trace;

use crate::fs::CWD;
use crate::hook::Hook;
use crate::languages::version::LanguageRequest;

static FINDER: LazyLock<Finder> = LazyLock::new(|| Finder::new(b"# /// script"));

/// A PEP 723 script, including its [`Pep723Metadata`].
#[derive(Debug, Clone)]
pub struct Pep723Script {
    /// The parsed [`Pep723Metadata`] table from the script.
    pub metadata: Pep723Metadata,
    /// The content of the script before the metadata table.
    pub prelude: String,
    /// The content of the script after the metadata table.
    pub postlude: String,
}

impl Pep723Script {
    /// Read the PEP 723 `script` metadata from a Python file, if it exists.
    ///
    /// Returns `None` if the file is missing a PEP 723 metadata block.
    ///
    /// See: <https://peps.python.org/pep-0723/>
    pub async fn read(file: impl AsRef<Path>) -> Result<Option<Self>, Pep723Error> {
        let contents = fs_err::tokio::read(&file).await?;

        // Extract the `script` tag.
        let ScriptTag {
            prelude,
            metadata,
            postlude,
        } = match ScriptTag::parse(&contents) {
            Ok(Some(tag)) => tag,
            Ok(None) => return Ok(None),
            Err(err) => return Err(err),
        };

        // Parse the metadata.
        let metadata = Pep723Metadata::from_str(&metadata)?;

        Ok(Some(Self {
            metadata,
            prelude,
            postlude,
        }))
    }
}

/// PEP 723 metadata as parsed from a `script` comment block.
///
/// See: <https://peps.python.org/pep-0723/>
#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "kebab-case")]
pub struct Pep723Metadata {
    pub dependencies: Option<Vec<String>>,
    pub requires_python: Option<String>,
}

impl FromStr for Pep723Metadata {
    type Err = toml::de::Error;

    /// Parse `Pep723Metadata` from a raw TOML string.
    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        let metadata = toml::from_str(raw)?;
        Ok(metadata)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Pep723Error {
    #[error(
        "An opening tag (`# /// script`) was found without a closing tag (`# ///`). Ensure that every line between the opening and closing tags (including empty lines) starts with a leading `#`."
    )]
    UnclosedBlock,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Utf8(#[from] std::str::Utf8Error),
    #[error(transparent)]
    Toml(#[from] toml::de::Error),
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ScriptTag {
    /// The content of the script before the metadata block.
    prelude: String,
    /// The metadata block.
    metadata: String,
    /// The content of the script after the metadata block.
    postlude: String,
}

impl ScriptTag {
    /// Given the contents of a Python file, extract the `script` metadata block with leading
    /// comment hashes removed, any preceding shebang or content (prelude), and the remaining Python
    /// script.
    ///
    /// Given the following input string representing the contents of a Python script:
    ///
    /// ```python
    /// #!/usr/bin/env python3
    /// # /// script
    /// # requires-python = '>=3.11'
    /// # dependencies = [
    /// #   'requests<3',
    /// #   'rich',
    /// # ]
    /// # ///
    ///
    /// import requests
    ///
    /// print("Hello, World!")
    /// ```
    ///
    /// This function would return:
    ///
    /// - Preamble: `#!/usr/bin/env python3\n`
    /// - Metadata: `requires-python = '>=3.11'\ndependencies = [\n  'requests<3',\n  'rich',\n]`
    /// - Postlude: `import requests\n\nprint("Hello, World!")\n`
    ///
    /// See: <https://peps.python.org/pep-0723/>
    pub fn parse(contents: &[u8]) -> Result<Option<Self>, Pep723Error> {
        // Identify the opening pragma.
        let Some(index) = FINDER.find(contents) else {
            return Ok(None);
        };

        // The opening pragma must be the first line, or immediately preceded by a newline.
        if !(index == 0 || matches!(contents[index - 1], b'\r' | b'\n')) {
            return Ok(None);
        }

        // Extract the preceding content.
        let prelude = std::str::from_utf8(&contents[..index])?;

        // Decode as UTF-8.
        let contents = &contents[index..];
        let contents = std::str::from_utf8(contents)?;

        let mut lines = contents.lines();

        // Ensure that the first line is exactly `# /// script`.
        if lines.next().is_none_or(|line| line != "# /// script") {
            return Ok(None);
        }

        // > Every line between these two lines (# /// TYPE and # ///) MUST be a comment starting
        // > with #. If there are characters after the # then the first character MUST be a space. The
        // > embedded content is formed by taking away the first two characters of each line if the
        // > second character is a space, otherwise just the first character (which means the line
        // > consists of only a single #).
        let mut toml = vec![];

        for line in lines {
            // Remove the leading `#`.
            let Some(line) = line.strip_prefix('#') else {
                break;
            };

            // If the line is empty, continue.
            if line.is_empty() {
                toml.push("");
                continue;
            }

            // Otherwise, the line _must_ start with ` `.
            let Some(line) = line.strip_prefix(' ') else {
                break;
            };

            toml.push(line);
        }

        // Find the closing `# ///`. The precedence is such that we need to identify the _last_ such
        // line.
        //
        // For example, given:
        // ```python
        // # /// script
        // #
        // # ///
        // #
        // # ///
        // ```
        //
        // The latter `///` is the closing pragma
        let Some(index) = toml.iter().rev().position(|line| *line == "///") else {
            return Err(Pep723Error::UnclosedBlock);
        };
        let index = toml.len() - index;

        // Discard any lines after the closing `# ///`.
        //
        // For example, given:
        // ```python
        // # /// script
        // #
        // # ///
        // #
        // #
        // ```
        //
        // We need to discard the last two lines.
        toml.truncate(index - 1);

        // Join the lines into a single string.
        let prelude = prelude.to_string();
        let metadata = toml.join("\n") + "\n";
        let postlude = contents
            .lines()
            .skip(index + 1)
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";

        Ok(Some(Self {
            prelude,
            metadata,
            postlude,
        }))
    }
}

pub(crate) async fn extract_pep723_metadata(hook: &mut Hook) -> Result<()> {
    if !hook.additional_dependencies.is_empty() {
        trace!(
            "Skipping reading PEP 723 metadata for hook `{hook}` because it already has `additional_dependencies`",
        );
        return Ok(());
    }

    // TODO: In workspace mode, local hooks should be relative to the project root.
    let repo_path = hook.repo_path().unwrap_or_else(|| CWD.as_path());

    let split = hook.entry.split()?;
    let file = repo_path.join(&split[0]);

    let Some(script) = Pep723Script::read(&file).await? else {
        return Ok(());
    };

    if let Some(dependencies) = script.metadata.dependencies {
        hook.additional_dependencies = dependencies.into_iter().collect();
    }
    if let Some(language_request) = script.metadata.requires_python {
        if !hook.language_request.is_any() {
            trace!(
                "`language_version` is ignored because `requires_python` is specified in the PEP 723 metadata"
            );
        }
        hook.language_request = LanguageRequest::parse(hook.language, &language_request)?;
    }

    Ok(())
}
