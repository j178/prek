use std::borrow::Cow;
use std::cmp::Ordering;
use std::ops::Range;
use std::path::Path;

use anyhow::{Result, bail};

use crate::hook::Hook;
use crate::hooks::pre_commit_hooks::{FilenamesArgs, parse_hook_args, run_file_checks};
use crate::run::INTERNAL_CONCURRENCY;

const BROKEN_PKG_RESOURCES: [&[u8]; 2] = [b"pkg-resources==0.0.0\n", b"pkg_resources==0.0.0\n"];

#[derive(Default)]
struct PendingRequirement<'a> {
    value: Option<Cow<'a, [u8]>>,
    comments: Vec<&'a [u8]>,
}

impl<'a> PendingRequirement<'a> {
    fn is_complete(&self) -> bool {
        self.value.as_deref().is_some_and(|value| {
            value
                .iter()
                .rev()
                .find(|&&byte| !matches!(byte, b'\r' | b'\n'))
                != Some(&b'\\')
        })
    }

    fn append_value(&mut self, line: &'a [u8]) {
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

    fn finish(self) -> Result<Requirement<'a>> {
        let Some(value) = self.value else {
            bail!("requirement has no value");
        };
        let name = requirement_name(&value);

        Ok(Requirement {
            value,
            comments: self.comments,
            name,
        })
    }
}

enum SortName {
    TopOfFile,
    Invalid,
    Bytes(Range<usize>),
}

struct Requirement<'a> {
    value: Cow<'a, [u8]>,
    comments: Vec<&'a [u8]>,
    name: SortName,
}

struct ParsedRequirements<'a> {
    requirements: Vec<Requirement<'a>>,
    trailing_comments: Vec<&'a [u8]>,
}

impl<'a> ParsedRequirements<'a> {
    fn parse(contents: &'a [u8]) -> Result<Self> {
        let mut requirements = Vec::new();
        let mut current = PendingRequirement::default();

        for line in contents.split_inclusive(|&byte| byte == b'\n') {
            if current.is_complete() {
                requirements.push(std::mem::take(&mut current).finish()?);
            }

            let is_first_requirement = requirements.is_empty();
            if is_first_requirement && line.trim_ascii().is_empty() {
                if current
                    .comments
                    .first()
                    .is_some_and(|comment| comment.starts_with(b"#"))
                {
                    // Upstream represents the separator after a top-of-file
                    // comment block as a synthetic newline, replacing any
                    // incomplete value accumulated before it.
                    current.value = Some(Cow::Borrowed(b"\n"));
                } else {
                    current.comments.push(line);
                }
            } else if line.trim_ascii_start().starts_with(b"#") || line.trim_ascii().is_empty() {
                current.comments.push(line);
            } else {
                current.append_value(line);
            }
        }

        let trailing_comments = if current.value.is_some() {
            requirements.push(current.finish()?);
            Vec::new()
        } else {
            current.comments
        };

        Ok(Self {
            requirements,
            trailing_comments,
        })
    }

    fn sort_and_filter(&mut self) -> Result<()> {
        self.requirements
            .retain(|requirement| !BROKEN_PKG_RESOURCES.contains(&requirement.value.as_ref()));

        let needs_name_comparison = self
            .requirements
            .iter()
            .filter(|requirement| !matches!(&requirement.name, SortName::TopOfFile))
            .nth(1)
            .is_some();

        // Upstream extracts names only while comparing entries, so a lone
        // entry without a sortable name is left unchanged.
        if !needs_name_comparison {
            return Ok(());
        }
        if self
            .requirements
            .iter()
            .any(|requirement| matches!(&requirement.name, SortName::Invalid))
        {
            bail!("requirement entry starts with whitespace or a semicolon");
        }

        self.requirements.sort_by(compare_requirements);
        Ok(())
    }

    fn render(&self, capacity: usize) -> Vec<u8> {
        let mut output = Vec::with_capacity(capacity);
        let mut previous = None;

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

pub(crate) async fn requirements_txt_fixer(
    hook: &Hook,
    filenames: &[&Path],
) -> Result<(i32, Vec<u8>)> {
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

async fn fix_file(file_base: &Path, filename: &Path) -> Result<(i32, Vec<u8>)> {
    let file_path = file_base.join(filename);
    let before = fs_err::tokio::read(&file_path).await?;

    let Some(after) = fixed_contents(&before)? else {
        return Ok((0, Vec::new()));
    };

    fs_err::tokio::write(file_path, after).await?;
    Ok((1, format!("Sorting {}\n", filename.display()).into_bytes()))
}

fn fixed_contents(before: &[u8]) -> Result<Option<Vec<u8>>> {
    // Upstream leaves empty and whitespace-only files byte-for-byte unchanged.
    if before.trim_ascii().is_empty() {
        return Ok(None);
    }

    let normalized = if before.ends_with(b"\n") {
        None
    } else {
        let mut normalized = Vec::with_capacity(before.len() + 1);
        normalized.extend_from_slice(before);
        normalized.push(b'\n');
        Some(normalized)
    };
    let contents = normalized.as_deref().unwrap_or(before);

    let mut parsed = ParsedRequirements::parse(contents)?;
    parsed.sort_and_filter()?;

    let after = parsed.render(contents.len());
    if after.as_slice() == before {
        Ok(None)
    } else {
        Ok(Some(after))
    }
}

fn requirement_name(value: &[u8]) -> SortName {
    if value == b"\n" {
        return SortName::TopOfFile;
    }

    for marker in [b"#egg=".as_slice(), b"&egg=".as_slice()] {
        if let Some(index) = find_subslice(value, marker) {
            return SortName::Bytes(index + marker.len()..value.len());
        }
    }

    let separator = value
        .iter()
        .position(|byte| *byte == b';' || byte.is_ascii_whitespace())
        .unwrap_or(value.len());
    if separator == 0 {
        return SortName::Invalid;
    }

    let comparison = (0..separator)
        .find(|&index| match value[index] {
            b'=' => value.get(index + 1) == Some(&b'='),
            b'!' | b'~' => value.get(index + 1) == Some(&b'='),
            b'>' | b'<' => true,
            _ => false,
        })
        .unwrap_or(separator);

    SortName::Bytes(0..comparison)
}

fn compare_requirements(left: &Requirement<'_>, right: &Requirement<'_>) -> Ordering {
    let names = match (&left.name, &right.name) {
        (SortName::TopOfFile, SortName::TopOfFile) => Ordering::Equal,
        (SortName::TopOfFile, _) => Ordering::Less,
        (_, SortName::TopOfFile) => Ordering::Greater,
        (SortName::Invalid, SortName::Invalid) => Ordering::Equal,
        (SortName::Invalid, SortName::Bytes(_)) => Ordering::Greater,
        (SortName::Bytes(_), SortName::Invalid) => Ordering::Less,
        (SortName::Bytes(left_name), SortName::Bytes(right_name)) => left.value[left_name.clone()]
            .iter()
            .map(u8::to_ascii_lowercase)
            .cmp(
                right.value[right_name.clone()]
                    .iter()
                    .map(u8::to_ascii_lowercase),
            ),
    };

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
    fn fixed_contents_matches_upstream_behavior() -> Result<()> {
        let cases: &[(&[u8], &[u8])] = &[
            (b"", b""),
            (b"\n", b"\n"),
            (b" \t", b" \t"),
            (b"  requests==2\n", b"  requests==2\n"),
            (b"# intentionally empty\n", b"# intentionally empty\n"),
            (
                b"# header\n\n  requests==2\n",
                b"# header\n\n  requests==2\n",
            ),
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
            let fixed = fixed_contents(before)?;
            assert_eq!(fixed.as_deref().unwrap_or(before), expected);
        }

        assert_eq!(
            fixed_contents(b"  requests==2\nflask\n")
                .unwrap_err()
                .to_string(),
            "requirement entry starts with whitespace or a semicolon"
        );

        Ok(())
    }
}
