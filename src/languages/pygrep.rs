use std::fmt::Write;
use std::process::Stdio;
use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

use crate::hook::{Hook, InstalledHook};
use crate::languages::LanguageImpl;
use crate::languages::python::Uv;
use crate::run::CONCURRENCY;
use crate::store::{CacheBucket, Store, ToolBucket};

#[derive(Debug, Default)]
struct Args {
    ignore_case: bool,
    multiline: bool,
    negate: bool,
}

impl Args {
    fn parse(args: &[String]) -> Result<Self> {
        let mut parsed = Args::default();

        for arg in args {
            match arg.as_str() {
                "--ignore-case" | "-i" => parsed.ignore_case = true,
                "--multiline" => parsed.multiline = true,
                "--negate" => parsed.negate = true,
                _ => anyhow::bail!("Unknown argument: {}", arg),
            }
        }

        Ok(parsed)
    }

    fn to_args(&self) -> Vec<&'static str> {
        fn as_str(value: bool) -> &'static str {
            if value { "1" } else { "0" }
        }
        vec![
            as_str(self.ignore_case),
            as_str(self.multiline),
            as_str(self.negate),
        ]
    }
}

pub(crate) struct Pygrep;

// We have to implement `pygrep` in Python, because Python `re` module has many differences
// from Rust `regex` crate.
static SCRIPT: &str = indoc::indoc! {r#"
import sys
import re
from re import Pattern
from concurrent.futures import ThreadPoolExecutor

output = sys.stdout.buffer

def process_file(
    filename: str, pattern: Pattern[bytes], multiline: bool, negate: bool
) -> int:
    if multiline:
        if negate:
            return _process_filename_at_once_negated(pattern, filename)
        else:
            return _process_filename_at_once(pattern, filename)
    else:
        if negate:
            return _process_filename_by_line_negated(pattern, filename)
        else:
            return _process_filename_by_line(pattern, filename)


def _process_filename_by_line(pattern: Pattern[bytes], filename: str) -> int:
    retv = 0
    with open(filename, "rb") as f:
        for line_no, line in enumerate(f, start=1):
            if pattern.search(line):
                retv = 1
                output.write(f"{filename}:{line_no}:".encode())
                output.write(line.rstrip(b"\r\n"))
                output.write(b"\n")
    return retv


def _process_filename_at_once(pattern: Pattern[bytes], filename: str) -> int:
    retv = 0
    with open(filename, "rb") as f:
        contents = f.read()
        match = pattern.search(contents)
        if match:
            retv = 1
            line_no = contents[: match.start()].count(b"\n")
            output.write(f"{filename}:{line_no + 1}:".encode())

            matched_lines = match[0].split(b"\n")
            matched_lines[0] = contents.split(b"\n")[line_no]

            output.write(b"\n".join(matched_lines))
            output.write(b"\n")
    return retv


def _process_filename_by_line_negated(
    pattern: Pattern[bytes],
    filename: str,
) -> int:
    with open(filename, "rb") as f:
        for line in f:
            if pattern.search(line):
                return 0
        else:
            output.write(filename.encode())
            output.write(b"\n")
            return 1


def _process_filename_at_once_negated(
    pattern: Pattern[bytes],
    filename: str,
) -> int:
    with open(filename, "rb") as f:
        contents = f.read()
    match = pattern.search(contents)
    if match:
        return 0
    else:
        output.write(filename.encode())
        output.write(b"\n")
        return 1


def main():
    ignore_case = sys.argv[1] == "1"
    multiline = sys.argv[2] == "1"
    negate = sys.argv[3] == "1"
    concurrency = int(sys.argv[4])
    pattern = sys.argv[5].encode()

    flags = re.IGNORECASE if ignore_case else 0
    if multiline:
        flags |= re.MULTILINE | re.DOTALL

    pattern = re.compile(pattern, flags)

    pool = ThreadPoolExecutor(max_workers=concurrency)
    futures = []

    for filename in sys.stdin.readlines():
        filename = filename.strip()
        futures.append(pool.submit(process_file, filename, pattern, multiline, negate))

    pool.shutdown(wait=True)

    ret = 0
    for future in futures:
        ret |= future.result()

    sys.exit(ret)


if __name__ == "__main__":
    main()
"#};

impl LanguageImpl for Pygrep {
    async fn install(&self, hook: Arc<Hook>, store: &Store) -> Result<InstalledHook> {
        let uv_dir = store.tools_path(ToolBucket::Uv);
        let uv = Uv::install(&uv_dir).await?;

        // Find or download a Python interpreter.

        Ok(InstalledHook::NoNeedInstall(hook))
    }

    async fn check_health(&self) -> Result<()> {
        todo!()
    }

    async fn run(
        &self,
        hook: &InstalledHook,
        filenames: &[&String],
        store: &Store,
    ) -> Result<(i32, Vec<u8>)> {
        let uv_dir = store.tools_path(ToolBucket::Uv);
        let uv = Uv::install(&uv_dir).await?;

        let py_script = tempfile::NamedTempFile::new_in(store.cache_path(CacheBucket::Python))?;
        fs_err::tokio::write(&py_script, SCRIPT)
            .await
            .context("Failed to write Python script")?;

        let args = Args::parse(&hook.args).context("Failed to parse arguments")?;
        let mut cmd = uv
            .cmd("uv run", store)
            .arg("run")
            .arg("python")
            .arg("-I") // Isolate mode.
            .arg("-B") // Don't write bytecode.
            .arg(py_script.path())
            .args(args.to_args())
            .arg(CONCURRENCY.to_string())
            .arg(hook.entry.entry())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .check(false)
            .spawn()?;

        let mut stdin = cmd.stdin.take().context("Failed to take stdin")?;

        let write_task = tokio::spawn(async move {
            let mut stdin = stdin;
            for filename in filenames {
                if let Err(e) = stdin.write_all(format!("{}\n", filename).as_bytes()).await {
                    break;
                }
            }
            let _ = stdin.shutdown().await;
        });

        let mut output = cmd
            .wait_with_output()
            .await
            .context("Failed to wait for command output")?;
        write_task.await.context("Failed to write stdin")?;

        output.stdout.extend(output.stderr);
        let code = output.status.code().unwrap_or(1);

        Ok((code, output.stdout))
    }
}
