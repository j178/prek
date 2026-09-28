use std::borrow::Cow;
use std::cmp::Ordering;
use std::ops::Range;
use std::path::Path;

use anyhow::Result;
use bstr::ByteSlice;

use crate::hook::Hook;
use crate::hooks::HookOutput;
use crate::hooks::pre_commit_hooks::{
    FixArgs, contents_equal, parse_hook_args, run_blocking_file_checks,
};

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
        let range = requirement_name(&value, self.line_number)?;
        let name = match &value {
            Cow::Borrowed(value) => {
                let name = &value[range];
                if name.iter().any(u8::is_ascii_uppercase) {
                    Cow::Owned(name.to_ascii_lowercase())
                } else {
                    Cow::Borrowed(name)
                }
            }
            Cow::Owned(value) => Cow::Owned(value[range].to_ascii_lowercase()),
        };

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
    name: Cow<'a, [u8]>,
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
        for (line_number, line) in contents.lines_with_terminator().enumerate() {
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

    fn chunks(&self) -> impl Iterator<Item = &[u8]> {
        let mut previous = None;
        let requirements = self.requirements.iter().flat_map(move |requirement| {
            let value = requirement.value.as_ref();
            let unique = if previous == Some(value) {
                None
            } else {
                Some(value)
            };
            previous = Some(value);
            requirement.comments.iter().copied().chain(unique)
        });

        self.header
            .iter()
            .copied()
            .chain(requirements)
            .chain(self.trailing_comments.iter().copied())
    }
}

/// Runs the `requirements-txt-fixer` hook.
pub(crate) async fn run(hook: &Hook, filenames: &[&Path]) -> Result<HookOutput> {
    let args: FixArgs = parse_hook_args(hook)?;

    run_blocking_file_checks(
        hook.project().relative_path(),
        &args.filenames,
        filenames,
        move |file_path, display_path| fix_file(file_path, display_path, args.check),
    )
    .await
}

fn fix_file(file_path: &Path, display_path: &Path, check: bool) -> Result<HookOutput> {
    let mut before = fs_err::read(file_path)?;
    let capacity = before.len() + 1;

    let fixed = match fixed_requirements(&mut before) {
        Ok(Some(fixed)) => fixed,
        Ok(None) => return Ok(HookOutput::unchanged(0, Vec::new())),
        Err(error) => {
            let output = format!(
                "{}:{}: {error}\n",
                display_path.display(),
                error.line_number()
            );
            return Ok(HookOutput::unchanged(1, output.into_bytes()));
        }
    };

    if check {
        return Ok(HookOutput::unchanged(
            1,
            format!("Would sort {}\n", display_path.display()).into_bytes(),
        ));
    }

    let mut after = Vec::with_capacity(capacity);
    for chunk in fixed.chunks() {
        after.extend_from_slice(chunk);
    }
    // Release the input and parsed entries before writing the rendered output.
    drop(fixed);
    drop(before);
    fs_err::write(file_path, after)?;
    Ok(HookOutput::known(
        1,
        format!("Sorting {}\n", display_path.display()).into_bytes(),
        true,
    ))
}

fn fixed_requirements(before: &mut Vec<u8>) -> FixResult<Option<ParsedRequirements<'_>>> {
    // Upstream leaves empty and whitespace-only files byte-for-byte unchanged.
    if before.trim_ascii().is_empty() {
        return Ok(None);
    }

    let original_len = before.len();
    if !before.ends_with(b"\n") {
        before.push(b'\n');
    }

    let mut parsed = ParsedRequirements::parse(before)?;
    parsed.sort_and_filter();

    if contents_equal(&before[..original_len], parsed.chunks()) {
        Ok(None)
    } else {
        Ok(Some(parsed))
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
        if let Some(index) =
            memchr::memchr_iter(marker[0], value).find(|&index| value[index..].starts_with(marker))
        {
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
    left.name
        .cmp(&right.name)
        .then_with(|| left.comments.is_empty().cmp(&right.comments.is_empty()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_requirements_match_expected_behavior() -> Result<()> {
        let cases: &[(&[u8], &[u8])] = &[
            (b"", b""),
            (b"\n", b"\n"),
            (b" \t", b" \t"),
            (b"# intentionally empty\n", b"# intentionally empty\n"),
            (b"foo\n# comment at end\n", b"foo\n# comment at end\n"),
            (
                b"pkg==2\n# uppercase\nPKG==1\nPkg==3\n",
                b"# uppercase\nPKG==1\npkg==2\nPkg==3\n",
            ),
            (
                b"Zoo==1 \\\n  --hash=sha256:abc\nalpha==2\n",
                b"alpha==2\nZoo==1 \\\n  --hash=sha256:abc\n",
            ),
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
                b"Beta\n-e git+https://url?x=1&egg=Zulu#fragment#egg=Alpha\n",
                b"-e git+https://url?x=1&egg=Zulu#fragment#egg=Alpha\nBeta\n",
            ),
            (
                b"Beta\n-e git+https://url?x=1&y=2&egg=Alpha\n",
                b"-e git+https://url?x=1&y=2&egg=Alpha\nBeta\n",
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
            let mut contents = before.to_vec();
            let fixed = fixed_requirements(&mut contents)?
                .map(|parsed| parsed.chunks().flatten().copied().collect::<Vec<_>>());
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
                fixed_requirements(&mut before.to_vec())
                    .err()
                    .map(|error| error.to_string())
                    .as_deref(),
                Some(expected)
            );
        }

        Ok(())
    }
}
