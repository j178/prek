use std::borrow::Cow;
use std::cmp::Ordering;
use std::ops::Range;
use std::path::Path;

use anyhow::Result;

use crate::hook::Hook;
use crate::hooks::HookOutput;
use crate::hooks::pre_commit_hooks::{FilenamesArgs, parse_hook_args, run_file_checks};
use crate::run::INTERNAL_CONCURRENCY;

const BROKEN_PKG_RESOURCES: [&[u8]; 2] = [b"pkg-resources==0.0.0\n", b"pkg_resources==0.0.0\n"];

#[derive(Default)]
struct PendingRequirement<'a> {
    value: Option<Cow<'a, [u8]>>,
    comments: Vec<&'a [u8]>,
    line_number: usize,
}

impl<'a> PendingRequirement<'a> {
    fn is_complete(&self) -> bool {
        // A trailing backslash keeps the logical requirement open for the next physical line.
        self.value.as_deref().is_some_and(|value| {
            value
                .iter()
                .rev()
                .find(|&&byte| !matches!(byte, b'\r' | b'\n'))
                != Some(&b'\\')
        })
    }

    fn append_value(&mut self, line: &'a [u8], line_number: usize) {
        // Continuation lines keep the first physical line as their diagnostic location.
        if self.value.is_none() {
            self.line_number = line_number;
        }
        self.value = Some(match self.value.take() {
            None => Cow::Borrowed(line),
            Some(Cow::Borrowed(previous)) => {
                let mut value = Vec::with_capacity(previous.len() + line.len());
                value.extend_from_slice(previous);
                value.extend_from_slice(line);
                Cow::Owned(value)
            }
            Some(Cow::Owned(mut value)) => {
                value.extend_from_slice(line);
                Cow::Owned(value)
            }
        });
    }

    fn take_requirement(&mut self) -> FixResult<Option<Requirement<'a>>> {
        let Some(value) = self.value.take() else {
            return Ok(None);
        };
        let name = requirement_name(&value, self.line_number)?;

        Ok(Some(Requirement {
            value,
            comments: std::mem::take(&mut self.comments),
            name,
        }))
    }
}

#[derive(Debug, thiserror::Error)]
enum InvalidRequirement {
    #[error("requirement entry starts with whitespace")]
    Whitespace(usize),
    #[error("requirement entry starts with a semicolon")]
    Semicolon(usize),
}

impl InvalidRequirement {
    fn line_number(&self) -> usize {
        match self {
            Self::Whitespace(line_number) | Self::Semicolon(line_number) => *line_number,
        }
    }
}

type FixResult<T> = std::result::Result<T, InvalidRequirement>;

struct Requirement<'a> {
    value: Cow<'a, [u8]>,
    comments: Vec<&'a [u8]>,
    name: Range<usize>,
}

struct ParsedRequirements<'a> {
    header: Vec<&'a [u8]>,
    requirements: Vec<Requirement<'a>>,
    trailing_comments: Vec<&'a [u8]>,
}

impl<'a> ParsedRequirements<'a> {
    fn parse(contents: &'a [u8]) -> FixResult<Self> {
        let mut header = Vec::new();
        let mut requirements = Vec::new();
        let mut current = PendingRequirement::default();

        // Comments and blank lines remain pending so they move with the following requirement.
        for (line_number, line) in contents.split_inclusive(|&byte| byte == b'\n').enumerate() {
            if current.is_complete() {
                if let Some(requirement) = current.take_requirement()? {
                    requirements.push(requirement);
                }
            }

            let is_blank = line.trim_ascii().is_empty();
            let at_start = header.is_empty() && requirements.is_empty();
            if at_start && is_blank {
                if current
                    .comments
                    .first()
                    .is_some_and(|comment| comment.starts_with(b"#"))
                {
                    // The first blank separator fixes the initial comment block at the top.
                    // Upstream also discards an incomplete value accumulated before it.
                    header = std::mem::take(&mut current.comments);
                    header.push(line);
                    current = PendingRequirement::default();
                } else {
                    current.comments.push(line);
                }
            } else if line.trim_ascii_start().starts_with(b"#") || is_blank {
                current.comments.push(line);
            } else {
                current.append_value(line, line_number + 1);
            }
        }

        // Comments with no following requirement remain at EOF instead of moving during sorting.
        let trailing_comments = match current.take_requirement()? {
            Some(requirement) => {
                requirements.push(requirement);
                Vec::new()
            }
            None => current.comments,
        };

        Ok(Self {
            header,
            requirements,
            trailing_comments,
        })
    }

    fn sort_and_filter(&mut self) {
        self.requirements
            .retain(|requirement| !BROKEN_PKG_RESOURCES.contains(&requirement.value.as_ref()));
        self.requirements.sort_by(compare_requirements);
    }

    fn render(&self, capacity: usize) -> Vec<u8> {
        let mut output = Vec::with_capacity(capacity);
        let mut previous = None;

        for &line in &self.header {
            output.extend_from_slice(line);
        }

        for requirement in &self.requirements {
            for &comment in &requirement.comments {
                output.extend_from_slice(comment);
            }

            let value = requirement.value.as_ref();
            if previous != Some(value) {
                output.extend_from_slice(value);
                previous = Some(value);
            }
        }

        for &comment in &self.trailing_comments {
            output.extend_from_slice(comment);
        }

        output
    }
}

/// Runs the `requirements-txt-fixer` hook.
pub(crate) async fn run(hook: &Hook, filenames: &[&Path]) -> Result<HookOutput> {
    let args: FilenamesArgs = parse_hook_args(hook)?;
    let file_base = hook.project().relative_path();

    run_file_checks(
        &args.filenames,
        filenames,
        *INTERNAL_CONCURRENCY,
        |filename| fix_file(file_base, filename),
    )
    .await
}

async fn fix_file(file_base: &Path, filename: &Path) -> Result<HookOutput> {
    let file_path = file_base.join(filename);
    let before = fs_err::tokio::read(&file_path).await?;

    let after = match fixed_contents(before) {
        Ok(Some(after)) => after,
        Ok(None) => return Ok(HookOutput::unchanged(0, Vec::new())),
        Err(error) => {
            let output = format!("{}:{}: {error}\n", filename.display(), error.line_number());
            return Ok(HookOutput::unchanged(1, output.into_bytes()));
        }
    };

    fs_err::tokio::write(file_path, after).await?;
    Ok(HookOutput::known(
        1,
        format!("Sorting {}\n", filename.display()).into_bytes(),
        true,
    ))
}

fn fixed_contents(mut before: Vec<u8>) -> FixResult<Option<Vec<u8>>> {
    // Upstream leaves empty and whitespace-only files byte-for-byte unchanged.
    if before.trim_ascii().is_empty() {
        return Ok(None);
    }

    let original_len = before.len();
    if !before.ends_with(b"\n") {
        before.push(b'\n');
    }

    let mut parsed = ParsedRequirements::parse(&before)?;
    parsed.sort_and_filter();

    let after = parsed.render(before.len());
    if after.as_slice() == &before[..original_len] {
        Ok(None)
    } else {
        Ok(Some(after))
    }
}

fn requirement_name(value: &[u8], line_number: usize) -> FixResult<Range<usize>> {
    match value.first() {
        Some(b';') => return Err(InvalidRequirement::Semicolon(line_number)),
        Some(byte) if byte.is_ascii_whitespace() => {
            return Err(InvalidRequirement::Whitespace(line_number));
        }
        _ => {}
    }

    for marker in [b"#egg=".as_slice(), b"&egg=".as_slice()] {
        if let Some(index) = find_subslice(value, marker) {
            return Ok(index + marker.len()..value.len());
        }
    }

    let separator = value
        .iter()
        .position(|byte| *byte == b';' || byte.is_ascii_whitespace())
        .unwrap_or(value.len());

    let comparison = (0..separator)
        .find(|&index| match value[index] {
            b'=' => value.get(index + 1) == Some(&b'='),
            b'!' | b'~' => value.get(index + 1) == Some(&b'='),
            b'>' | b'<' => true,
            _ => false,
        })
        .unwrap_or(separator);

    Ok(0..comparison)
}

fn compare_requirements(left: &Requirement<'_>, right: &Requirement<'_>) -> Ordering {
    let names = left.value[left.name.clone()]
        .iter()
        .map(u8::to_ascii_lowercase)
        .cmp(
            right.value[right.name.clone()]
                .iter()
                .map(u8::to_ascii_lowercase),
        );

    names.then_with(|| left.comments.is_empty().cmp(&right.comments.is_empty()))
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_contents_matches_expected_behavior() -> Result<()> {
        let cases: &[(&[u8], &[u8])] = &[
            (b"", b""),
            (b"\n", b"\n"),
            (b" \t", b" \t"),
            (b"# intentionally empty\n", b"# intentionally empty\n"),
            (b"foo\n# comment at end\n", b"foo\n# comment at end\n"),
            (b"foo\nbar\n", b"bar\nfoo\n"),
            (b"bar\nfoo\n", b"bar\nfoo\n"),
            (b"a\nc\nb\n", b"a\nb\nc\n"),
            (b"a\nc\nb", b"a\nb\nc\n"),
            (b"a\nb\nc", b"a\nb\nc\n"),
            (
                b"#comment1\nfoo\n#comment2\nbar\n",
                b"#comment2\nbar\n#comment1\nfoo\n",
            ),
            (
                b"#comment1\nbar\n#comment2\nfoo\n",
                b"#comment1\nbar\n#comment2\nfoo\n",
            ),
            (b"#comment\n\nfoo\nbar\n", b"#comment\n\nbar\nfoo\n"),
            (b"#comment\n\nbar\nfoo\n", b"#comment\n\nbar\nfoo\n"),
            (
                b"foo\n\t#comment with indent\nbar\n",
                b"\t#comment with indent\nbar\nfoo\n",
            ),
            (
                b"bar\n\t#comment with indent\nfoo\n",
                b"bar\n\t#comment with indent\nfoo\n",
            ),
            (b"\nfoo\nbar\n", b"bar\n\nfoo\n"),
            (b"\nbar\nfoo\n", b"\nbar\nfoo\n"),
            (
                b"pyramid-foo==1\npyramid>=2\n",
                b"pyramid>=2\npyramid-foo==1\n",
            ),
            (
                b"a==1\nc>=1\nbbbb!=1\nc-a>=1;python_version>=\"3.6\"\ne>=2\nd>2\ng<2\nf<=2\n",
                b"a==1\nbbbb!=1\nc>=1\nc-a>=1;python_version>=\"3.6\"\nd>2\ne>=2\nf<=2\ng<2\n",
            ),
            (b"a==1\nb==1\na==1\n", b"a==1\nb==1\n"),
            (
                b"a==1\nb==1\n#comment about a\na==1\n",
                b"#comment about a\na==1\nb==1\n",
            ),
            (
                b"ocflib\nDjango\nPyMySQL\n",
                b"Django\nocflib\nPyMySQL\n",
            ),
            (
                b"-e git+ssh://git_url@tag#egg=ocflib\nDjango\nPyMySQL\n",
                b"Django\n-e git+ssh://git_url@tag#egg=ocflib\nPyMySQL\n",
            ),
            (
                b"bar\npkg-resources==0.0.0\nfoo\n",
                b"bar\nfoo\n",
            ),
            (
                b"foo\npkg-resources==0.0.0\nbar\n",
                b"bar\nfoo\n",
            ),
            (
                b"bar\npkg_resources==0.0.0\nfoo\n",
                b"bar\nfoo\n",
            ),
            (
                b"foo\npkg_resources==0.0.0\nbar\n",
                b"bar\nfoo\n",
            ),
            (
                b"git+ssh://git_url@tag#egg=ocflib\nDjango\nijk\n",
                b"Django\nijk\ngit+ssh://git_url@tag#egg=ocflib\n",
            ),
            (
                b"b==1.0.0\nc=2.0.0 \\\n --hash=sha256:abcd\na=3.0.0 \\\n  --hash=sha256:a1b1c1d1",
                b"a=3.0.0 \\\n  --hash=sha256:a1b1c1d1\nb==1.0.0\nc=2.0.0 \\\n --hash=sha256:abcd\n",
            ),
            (
                b"a=2.0.0 \\\n --hash=sha256:abcd\nb==1.0.0\n",
                b"a=2.0.0 \\\n --hash=sha256:abcd\nb==1.0.0\n",
            ),
            (b"foo\r\nbar\r\n", b"bar\r\nfoo\r\n"),
            (b"# header\nfoo \\\n\nbar\n", b"# header\n\nbar\n"),
            (
                b"zeta\n-e git+ssh://url \\\n --config=#egg=Alpha\n",
                b"-e git+ssh://url \\\n --config=#egg=Alpha\nzeta\n",
            ),
            (
                b"b\na=1 \\\n --hash=x\na=1 \\\n --hash=x\n",
                b"a=1 \\\n --hash=x\nb\n",
            ),
        ];

        for &(before, expected) in cases {
            let fixed = fixed_contents(before.to_vec())?;
            assert_eq!(fixed.as_deref().unwrap_or(before), expected);
        }

        for &(before, expected) in &[
            (
                b"  requests==2\n".as_slice(),
                "requirement entry starts with whitespace",
            ),
            (
                b";requests==2\n".as_slice(),
                "requirement entry starts with a semicolon",
            ),
        ] {
            assert_eq!(
                fixed_contents(before.to_vec()).unwrap_err().to_string(),
                expected
            );
        }

        Ok(())
    }
}
