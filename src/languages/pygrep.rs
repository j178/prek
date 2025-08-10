use std::process::Stdio;
use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::io::AsyncWriteExt;

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

#[derive(serde::Deserialize, thiserror::Error, Debug)]
#[serde(tag = "type")]
enum Error {
    #[error("Failed to parse regex: {message}")]
    Regex { message: String },
    #[error("IO error: {message}")]
    IO{ message: String },
    #[error("Unknown error: {message}")]
    Unknown{ message: String },
}

// We have to implement `pygrep` in Python, because Python `re` module has many differences
// from Rust `regex` crate.
static SCRIPT: &str = indoc::indoc! {r#"
import json
import sys
import re
from re import Pattern
from concurrent.futures import ThreadPoolExecutor
from queue import Queue

def process_file(
    filename: str, pattern: Pattern[bytes], multiline: bool, negate: bool, queue: Queue
) -> int:
    if multiline:
        if negate:
            ret, output = _process_filename_at_once_negated(pattern, filename)
        else:
            ret, output = _process_filename_at_once(pattern, filename)
    else:
        if negate:
            ret, output = _process_filename_by_line_negated(pattern, filename)
        else:
            ret, output = _process_filename_by_line(pattern, filename)
    queue.put((ret, output))

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


def run(ignore_case: bool, multiline: bool, negate: bool, concurrency: int, pattern: bytes):
    flags = re.IGNORECASE if ignore_case else 0
    if multiline:
        flags |= re.MULTILINE | re.DOTALL
    pattern = re.compile(pattern, flags)

    queue = Queue()
    pool = ThreadPoolExecutor(max_workers=concurrency)

    def producer():
        for line in sys.stdin:
            pool.submit(process_file, line.strip(), pattern, multiline, negate)


    def consumer():
        while True:
            try:
                ret, output = queue.get()
                if ret != 0 or output:
                    sys.stdout.buffer.write(output)
            except Exception:
                break

    t1 = Thread(target=producer)
    t2 = Thread(target=consumer)
    t1.start()
    t2.start()

    pool.shutdown(wait=True)

    retv = 0
    while not queue.empty():
        ret, output = queue.get()
        retv |= ret
        if output:
            sys.stdout.buffer.write(output)

    sys.stderr.buffer.write('{"code": retv}'.encode())


def main():
    ignore_case = sys.argv[1] == "1"
    multiline = sys.argv[2] == "1"
    negate = sys.argv[3] == "1"
    concurrency = int(sys.argv[4])
    pattern = sys.argv[5].encode()

    try:
        run(ignore_case, multiline, negate, concurrency, pattern)
    except re.error as e:
        error = {"type": "Regex", "message": str(e)}
        sys.stderr.buffer.write(json.dumps(error).encode())
        sys.exit(1)
    except OSError as e:
        error = {"type": "IO", "message": str(e)}
        sys.stderr.buffer.write(json.dumps(error).encode())
        sys.exit(1)
    except Exception as e:
        error = {"type": "Unknown", "message": str(e)}
        sys.stderr.buffer.write(json.dumps(error).encode())
        sys.exit(1)

if __name__ == "__main__":
    main()
"#};

pub(crate) struct Pygrep;

impl LanguageImpl for Pygrep {
    async fn install(&self, hook: Arc<Hook>, store: &Store) -> Result<InstalledHook> {
        let uv_dir = store.tools_path(ToolBucket::Uv);
        let _uv = Uv::install(&uv_dir).await?;

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

        let cache = store.cache_path(CacheBucket::Python);
        fs_err::tokio::create_dir_all(&cache).await?;

        let py_script = tempfile::NamedTempFile::new_in(cache)?;
        fs_err::tokio::write(&py_script, SCRIPT)
            .await
            .context("Failed to write Python script")?;

        let args = Args::parse(&hook.args).context("Failed to parse `args`")?;
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
        // TODO: avoid this clone if possible.
        let filenames: Vec<_> = filenames.iter().map(ToString::to_string).collect();

        let write_task = tokio::spawn(async move {
            for filename in filenames {
                stdin.write_all(format!("{filename}\n").as_bytes()).await?;
            }
            let _ = stdin.shutdown().await;
            anyhow::Ok(())
        });

        let output = cmd
            .wait_with_output()
            .await
            .context("Failed to wait for command output")?;
        write_task.await.context("Failed to write stdin")??;

        let code = output.status.code().unwrap_or(1);
        if code == 2 {
            // println!("Error output: {}", String::from_utf8_lossy(&output.stdout));
            let err: Error = serde_json::from_slice(output.stdout.as_slice())
                .context("Failed to parse error output")?;
            return Err(err.into());
        }

        Ok((code, output.stdout))
    }
}
