// Copyright (c) 2023 Axo Developer Co.
//
// Permission is hereby granted, free of charge, to any
// person obtaining a copy of this software and associated
// documentation files (the "Software"), to deal in the
// Software without restriction, including without
// limitation the rights to use, copy, modify, merge,
// publish, distribute, sublicense, and/or sell copies of
// the Software, and to permit persons to whom the Software
// is furnished to do so, subject to the following
// conditions:
//
// The above copyright notice and this permission notice
// shall be included in all copies or substantial portions
// of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
// ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
// TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
// PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
// SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
// CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
// OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
// IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
// DEALINGS IN THE SOFTWARE.

/// Adapt [axoprocess] to use [`tokio::process::Process`] instead of [`std::process::Command`].
use std::ffi::OsStr;
use std::fmt::Display;
use std::ops::Range;
use std::path::Path;
use std::process::Output;
use std::process::{CommandArgs, CommandEnvs, ExitStatus, Stdio};

use owo_colors::OwoColorize;
use thiserror::Error;
use tokio::io::AsyncReadExt;
use tracing::{enabled, trace};

/// An error from executing a command.
#[derive(Debug, Error)]
pub enum Error {
    /// The command could not be started or monitored to completion.
    #[error("Failed to run `{command}`")]
    Exec {
        /// The command that failed.
        command: String,
        /// What failed.
        #[source]
        cause: std::io::Error,
    },
    #[error("Command `{command}` exited with an error:\n{error}")]
    Status { command: String, error: StatusError },
    #[cfg(not(windows))]
    #[error("Failed to open pty")]
    Pty(#[from] prek_pty::Error),
    #[error("Failed to setup subprocess for pty")]
    PtySetup(#[from] std::io::Error),
}

/// The command ran but signaled an error condition through its exit status.
#[derive(Debug)]
pub struct StatusError {
    pub status: ExitStatus,
    pub output: Option<Output>,
}

impl Display for StatusError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "\n{}\n{}", "[status]".red(), self.status)?;

        if let Some(output) = &self.output {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            write_trimmed_output_section(f, "[stdout]", &stdout)?;
            write_trimmed_output_section(f, "[stderr]", &stderr)?;
        }

        Ok(())
    }
}

fn write_trimmed_output_section(
    f: &mut std::fmt::Formatter<'_>,
    label: &str,
    output: &str,
) -> std::fmt::Result {
    let mut lines = output.split('\n').filter_map(|line| {
        let line = line.trim();
        if line.is_empty() { None } else { Some(line) }
    });

    let Some(first) = lines.next() else {
        return Ok(());
    };

    writeln!(f, "\n{}\n{}", label.red(), first)?;
    for line in lines {
        writeln!(f, "{line}")?;
    }
    Ok(())
}

/// A fancier Command, see the crate's top-level docs!
pub struct Cmd {
    /// The inner command, in case you need to access it.
    pub inner: tokio::process::Command,
    hidden_arg_ranges: Vec<Range<usize>>,
    file_arg_boundary: usize,
    check_status: bool,
}

pub(crate) trait OutputSink {
    fn write_chunk(&mut self, chunk: &[u8]);
}

fn write_output_chunk(output: &mut Vec<u8>, sink: &mut impl OutputSink, chunk: &[u8]) {
    output.extend_from_slice(chunk);
    sink.write_chunk(chunk);
}

/// Constructors
impl Cmd {
    /// Create a new command.
    pub fn new(command: impl AsRef<OsStr>) -> Self {
        let inner = tokio::process::Command::new(command);
        Self {
            inner,
            hidden_arg_ranges: Vec::new(),
            file_arg_boundary: usize::MAX,
            check_status: true,
        }
    }
}

/// Builder APIs
impl Cmd {
    /// Pipe stdout into stderr
    ///
    /// This is useful for cases where you want your program to livestream
    /// the output of a command to give your user realtime feedback, but the command
    /// randomly writes some things to stdout, and you don't want your own stdout tainted.
    pub fn stdout_to_stderr(&mut self) -> &mut Self {
        self.inner.stdout(std::io::stderr());

        self
    }

    /// Set whether `ExitStatus::success` should be checked after executions
    /// (except `spawn`, which doesn't yet have an exit status to check).
    ///
    /// Defaults to `true`.
    ///
    /// If true, a non-zero exit status will produce an error.
    ///
    /// Execution methods that return or capture an exit status use this setting.
    pub fn check(&mut self, checked: bool) -> &mut Self {
        self.check_status = checked;
        self
    }
}

/// Execution APIs
impl Cmd {
    /// Equivalent to [`Cmd::status`],
    /// but doesn't bother returning the actual status code (because it's captured in the Result)
    pub async fn run(&mut self) -> Result<(), Error> {
        self.status().await?;
        Ok(())
    }

    /// Equivalent to [`std::process::Command::spawn`],
    /// but logged and with the error wrapped.
    pub fn spawn(&mut self) -> Result<tokio::process::Child, Error> {
        self.log_command();
        self.inner.spawn().map_err(|cause| self.exec_error(cause))
    }

    /// Equivalent to [`std::process::Command::output`],
    /// but logged, with the error wrapped, and status checked (by default)
    pub async fn output(&mut self) -> Result<Output, Error> {
        self.log_command();
        let output = self
            .inner
            .output()
            .await
            .map_err(|cause| self.exec_error(cause))?;
        self.maybe_check_output(output)
    }

    /// Like [`Cmd::output`], but streams stdout and stderr chunks into `sink` as
    /// they are read. The sink receives both pipes in arrival order; the returned
    /// output keeps stdout and stderr separated.
    pub(crate) async fn output_with_sink<S: OutputSink>(
        &mut self,
        mut sink: S,
    ) -> Result<Output, Error> {
        self.log_command();
        self.inner.stdin(Stdio::null());
        self.inner.stdout(Stdio::piped());
        self.inner.stderr(Stdio::piped());

        let mut child = self.inner.spawn().map_err(|cause| self.exec_error(cause))?;

        let mut stdout = child
            .stdout
            .take()
            .expect("child stdout must be piped before spawn");
        let mut stderr = child
            .stderr
            .take()
            .expect("child stderr must be piped before spawn");
        let mut stdout_done = false;
        let mut stderr_done = false;
        let mut stdout_buffer = [0u8; 4096];
        let mut stderr_buffer = [0u8; 4096];
        let mut stdout_output = Vec::new();
        let mut stderr_output = Vec::new();

        while !stdout_done || !stderr_done {
            tokio::select! {
                result = stdout.read(&mut stdout_buffer), if !stdout_done => {
                    match result {
                        Ok(0) => stdout_done = true,
                        Ok(n) => write_output_chunk(&mut stdout_output, &mut sink, &stdout_buffer[..n]),
                        Err(cause) => {
                            return Err(self.exec_error(cause));
                        }
                    }
                }
                result = stderr.read(&mut stderr_buffer), if !stderr_done => {
                    match result {
                        Ok(0) => stderr_done = true,
                        Ok(n) => write_output_chunk(&mut stderr_output, &mut sink, &stderr_buffer[..n]),
                        Err(cause) => {
                            return Err(self.exec_error(cause));
                        }
                    }
                }
            }
        }

        // For regular pipes, EOF on both streams is the point where output capture is complete.
        // Waiting earlier must not make us return before trailing pipe bytes are read.
        let status = child.wait().await.map_err(|cause| self.exec_error(cause))?;
        let output = Output {
            status,
            stdout: stdout_output,
            stderr: stderr_output,
        };

        self.maybe_check_output(output)
    }

    #[cfg(windows)]
    pub(crate) async fn pty_output_with_sink<S: OutputSink>(
        &mut self,
        sink: S,
    ) -> Result<Output, Error> {
        self.output_with_sink(sink).await
    }

    #[cfg(not(windows))]
    pub(crate) async fn pty_output_with_sink<S: OutputSink>(
        &mut self,
        sink: S,
    ) -> Result<Output, Error> {
        // If color is not used, fallback to piped output.
        if !*crate::run::USE_COLOR {
            return self.output_with_sink(sink).await;
        }

        self.run_on_pty(sink).await
    }

    #[cfg(not(windows))]
    async fn run_on_pty<S: OutputSink>(&mut self, mut sink: S) -> Result<Output, Error> {
        let (mut pty, pts) = prek_pty::open()?;
        let (_, stdout, stderr) = pts.setup_subprocess()?;

        self.inner.stdin(Stdio::null());
        self.inner.stdout(stdout);
        self.inner.stderr(stderr);

        // We run some commands under a PTY so they behave like they do in an interactive terminal
        // (colors, progress bars, etc.). However, this is still a *pseudo*-terminal and it doesn't
        // necessarily provide a full/accurate terminal environment.
        //
        // Some libraries (for example Go's termenv) send OSC/CSI queries and wait for a response
        // from the terminal. Our PTY doesn't emulate those responses, so they can block on a
        // timeout if the program insists on probing capabilities.
        //
        // Previously, we tried to work around this by setting `TERM=dumb` in the environment,
        // but that caused other issues (for example, some programs (e.g cargo), disable color entirely when they see `TERM=dumb`,
        // even if the output is actually a terminal that supports color).
        //
        // We intentionally do not make the child a session leader/foreground process group here.
        // When we did, termenv detected it as foreground and ran OSC probes, which then hung.

        let mut child = self.spawn()?;
        // The parent must not keep the slave side open; otherwise EOF no longer
        // represents only the child-side descriptors closing.
        drop(pts);

        let mut buffer = [0u8; 4096];
        let mut output = Vec::new();

        let status = loop {
            tokio::select! {
                read_result = pty.read(&mut buffer) => {
                    match read_result {
                        Ok(0) => break child.wait().await.map_err(|cause| self.exec_error(cause))?,
                        Ok(n) => write_output_chunk(&mut output, &mut sink, &buffer[..n]),
                        // Linux reports PTY master EOF as EIO after all slave handles close.
                        Err(err) if err.raw_os_error() == Some(libc::EIO) => {
                            break child.wait().await.map_err(|cause| self.exec_error(cause))?;
                        }
                        Err(err) => return Err(Error::PtySetup(err)),
                    }
                }
                status = child.wait() => {
                    let status = status.map_err(|cause| self.exec_error(cause))?;
                    // Child exit can be observed before the PTY read future is woken. Drain any
                    // bytes already available so fast commands do not lose their final output.
                    loop {
                        match pty.try_read(&mut buffer) {
                            Ok(0) => break,
                            Ok(n) => write_output_chunk(&mut output, &mut sink, &buffer[..n]),
                            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => break,
                            // Linux reports PTY master EOF as EIO after all slave handles close.
                            Err(err) if err.raw_os_error() == Some(libc::EIO) => break,
                            Err(err) => return Err(Error::PtySetup(err)),
                        }
                    }
                    break status;
                }
            }
        };

        child.stdin.take();
        child.stdout.take();
        child.stderr.take();

        let output = Output {
            status,
            stdout: output,
            stderr: Vec::new(),
        };

        self.maybe_check_output(output)
    }

    /// Equivalent to [`std::process::Command::status`]
    /// but logged, with the error wrapped, and status checked (by default)
    pub async fn status(&mut self) -> Result<ExitStatus, Error> {
        self.log_command();
        let status = self
            .inner
            .status()
            .await
            .map_err(|cause| self.exec_error(cause))?;
        self.maybe_check_status(status)?;
        Ok(status)
    }
}

/// Selected forwarded [`std::process::Command`] APIs.
impl Cmd {
    /// Forwards to [`std::process::Command::arg`].
    pub fn arg<S: AsRef<OsStr>>(&mut self, arg: S) -> &mut Self {
        self.inner.arg(arg);
        self
    }

    /// Forwards to [`std::process::Command::args`].
    pub fn args<I, S>(&mut self, args: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.inner.args(args);
        self
    }

    /// Append arguments without showing them in display and error messages.
    pub fn hidden_args<I, S>(&mut self, args: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let start = self.get_args().count();
        self.inner.args(args);
        let end = self.get_args().count();
        if start < end {
            if let Some(last) = self.hidden_arg_ranges.last_mut()
                && last.end >= start
            {
                last.end = last.end.max(end);
                return self;
            }
            self.hidden_arg_ranges.push(start..end);
        }
        self
    }

    /// Append trailing file-list arguments without showing them in display, error messages, or logs.
    pub fn file_args<I, S>(&mut self, args: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        if self.file_arg_boundary != usize::MAX {
            self.inner.args(args);
            return self;
        }

        let mut args = args.into_iter().peekable();
        if args.peek().is_none() {
            return self;
        }

        let start = self.get_args().count();
        self.inner.args(args);
        self.file_arg_boundary = start;
        self
    }

    /// Forwards to [`std::process::Command::env`].
    pub fn env<K, V>(&mut self, key: K, val: V) -> &mut Self
    where
        K: AsRef<OsStr>,
        V: AsRef<OsStr>,
    {
        self.inner.env(key, val);
        self
    }

    /// Forwards to [`std::process::Command::envs`].
    pub fn envs<I, K, V>(&mut self, vars: I) -> &mut Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<OsStr>,
        V: AsRef<OsStr>,
    {
        self.inner.envs(vars);
        self
    }

    /// Forwards to [`std::process::Command::env_remove`].
    pub fn env_remove<K: AsRef<OsStr>>(&mut self, key: K) -> &mut Self {
        self.inner.env_remove(key);
        self
    }

    /// Forwards to [`std::process::Command::env_clear`].
    pub fn env_clear(&mut self) -> &mut Self {
        self.inner.env_clear();
        self
    }

    /// Forwards to [`std::process::Command::current_dir`].
    pub fn current_dir<P: AsRef<Path>>(&mut self, dir: P) -> &mut Self {
        self.inner.current_dir(dir);
        self
    }

    /// Forwards to [`std::process::Command::stdin`].
    pub fn stdin<T: Into<Stdio>>(&mut self, cfg: T) -> &mut Self {
        self.inner.stdin(cfg);
        self
    }

    /// Forwards to [`std::process::Command::stdout`].
    pub fn stdout<T: Into<Stdio>>(&mut self, cfg: T) -> &mut Self {
        self.inner.stdout(cfg);
        self
    }

    /// Forwards to [`std::process::Command::stderr`].
    pub fn stderr<T: Into<Stdio>>(&mut self, cfg: T) -> &mut Self {
        self.inner.stderr(cfg);
        self
    }

    /// Forwards to [`std::process::Command::get_program`].
    pub fn get_program(&self) -> &OsStr {
        self.inner.as_std().get_program()
    }

    /// Forwards to [`std::process::Command::get_args`].
    pub fn get_args(&self) -> CommandArgs<'_> {
        self.inner.as_std().get_args()
    }

    /// Forwards to [`std::process::Command::get_envs`].
    pub fn get_envs(&self) -> CommandEnvs<'_> {
        self.inner.as_std().get_envs()
    }

    /// Forwards to [`std::process::Command::get_current_dir`].
    pub fn get_current_dir(&self) -> Option<&Path> {
        self.inner.as_std().get_current_dir()
    }

    /// Remove some git-specific environment variables to make git commands isolated.
    pub fn remove_git_envs(&mut self) -> &mut Self {
        for (key, _) in crate::git::GIT_ENV_TO_REMOVE.iter() {
            self.inner.env_remove(key);
        }
        self
    }
}

/// Diagnostic APIs used by execution methods and direct child-process callers.
impl Cmd {
    fn exec_error(&self, cause: std::io::Error) -> Error {
        let mut command = String::new();
        let _ = write_command_line(&mut command, None, self.get_program(), self.display_args());
        Error::Exec { command, cause }
    }

    fn status_error(&self, status: ExitStatus, output: Option<Output>) -> Error {
        let mut command = String::new();
        let _ = write_command_line(&mut command, None, self.get_program(), self.display_args());
        Error::Status {
            command,
            error: StatusError { status, output },
        }
    }

    /// Check `ExitStatus::success`, producing a contextual error if it's `false`.
    pub fn check_status(&self, status: ExitStatus) -> Result<(), Error> {
        if status.success() {
            Ok(())
        } else {
            Err(self.status_error(status, None))
        }
    }

    /// Check `Output::status`, producing a contextual error if it's not successful.
    pub fn check_output(&self, output: Output) -> Result<Output, Error> {
        if output.status.success() {
            Ok(output)
        } else {
            Err(self.status_error(output.status, Some(output)))
        }
    }

    /// Invoke [`Cmd::check_status`] if [`Cmd::check`] is `true`
    /// (defaults to `true`).
    pub fn maybe_check_status(&self, status: ExitStatus) -> Result<(), Error> {
        if self.check_status {
            self.check_status(status)?;
        }
        Ok(())
    }

    /// Invoke [`Cmd::check_output`] if [`Cmd::check`] is `true`
    /// (defaults to `true`).
    pub fn maybe_check_output(&self, output: Output) -> Result<Output, Error> {
        if self.check_status {
            self.check_output(output)
        } else {
            Ok(output)
        }
    }

    /// Log the current command with [`tracing::trace!`].
    pub fn log_command(&self) {
        if !enabled!(tracing::Level::TRACE) {
            return;
        }

        let mut command = String::new();
        let _ = write_command_line(
            &mut command,
            self.get_current_dir(),
            self.get_program(),
            self.non_file_args(),
        );
        trace!("Executing `{command}`");
    }

    fn display_args(&self) -> impl Iterator<Item = &OsStr> {
        self.non_file_args().enumerate().filter_map(|(index, arg)| {
            if self.is_hidden_arg(index) {
                None
            } else {
                Some(arg)
            }
        })
    }

    fn is_hidden_arg(&self, index: usize) -> bool {
        self.hidden_arg_ranges
            .iter()
            .any(|range| range.contains(&index))
    }

    fn non_file_args(&self) -> impl Iterator<Item = &OsStr> {
        self.get_args().take(self.file_arg_boundary)
    }
}

/// Simplified command output, omitting hidden arguments and file-list arguments.
impl Display for Cmd {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write_command_line(
            f,
            self.get_current_dir(),
            self.get_program(),
            self.display_args(),
        )
    }
}

fn write_command_line<'a>(
    f: &mut impl std::fmt::Write,
    cwd: Option<&Path>,
    program: &OsStr,
    args: impl IntoIterator<Item = &'a OsStr>,
) -> std::fmt::Result {
    if let Some(cwd) = cwd {
        write!(f, "cd {} && ", cwd.to_string_lossy())?;
    }

    let program_display = program.to_string_lossy();
    write!(f, "{program_display}")?;
    let mut args = args.into_iter().peekable();
    if args.peek().is_some_and(|arg| *arg == program) {
        args.next(); // Skip the program if it's repeated
    }

    for arg in args {
        write!(f, " {}", arg.to_string_lossy())?;
    }

    Ok(())
}

#[cfg(all(test, not(windows)))]
mod tests {
    use std::error::Error as _;
    use std::sync::{Arc, Mutex};

    use super::{Cmd, OutputSink, write_command_line};

    #[derive(Default)]
    struct RecordingSink {
        chunks: Arc<Mutex<usize>>,
    }

    impl OutputSink for RecordingSink {
        fn write_chunk(&mut self, _chunk: &[u8]) {
            *self.chunks.lock().unwrap() += 1;
        }
    }

    fn command_log_string(cmd: &Cmd) -> String {
        let mut command = String::new();
        let _ = write_command_line(
            &mut command,
            cmd.get_current_dir(),
            cmd.get_program(),
            cmd.non_file_args(),
        );
        command
    }

    #[tokio::test]
    async fn status_reports_missing_executable_name() {
        let err = Cmd::new("__prek_missing_command__")
            .status()
            .await
            .expect_err("command should not exist");

        assert_eq!(err.to_string(), "Failed to run `__prek_missing_command__`");
        let source = err.source().expect("missing executable error has source");
        let io_error = source
            .downcast_ref::<std::io::Error>()
            .expect("source is an io error");
        assert_eq!(io_error.kind(), std::io::ErrorKind::NotFound);
    }

    #[test]
    fn display_and_log_commands_omit_file_args() {
        let mut cmd = Cmd::new("prek");
        cmd.arg("run")
            .arg("hook-id")
            .file_args(["file-0.rs"])
            .file_args(["file-1.rs"]);

        assert_eq!(cmd.to_string(), "prek run hook-id");
        assert_eq!(command_log_string(&cmd), "prek run hook-id");
        assert_eq!(
            cmd.get_args()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
            ["run", "hook-id", "file-0.rs", "file-1.rs"]
        );
        assert!(cmd.hidden_arg_ranges.is_empty());
        assert_eq!(cmd.file_arg_boundary, 2);
    }

    #[test]
    fn display_command_omits_hidden_args() {
        let mut cmd = Cmd::new("git");
        cmd.hidden_args(["-c", "core.useBuiltinFSMonitor=false"])
            .arg("diff")
            .arg("--name-only")
            .hidden_args(["--no-ext-diff", "--ignore-submodules"])
            .arg("HEAD");

        assert_eq!(cmd.to_string(), "git diff --name-only HEAD");
        assert_eq!(
            cmd.get_args()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
            [
                "-c",
                "core.useBuiltinFSMonitor=false",
                "diff",
                "--name-only",
                "--no-ext-diff",
                "--ignore-submodules",
                "HEAD"
            ]
        );
    }

    #[test]
    fn command_log_includes_hidden_args() {
        let mut cmd = Cmd::new("git");
        cmd.hidden_args(["-c", "core.useBuiltinFSMonitor=false"])
            .arg("diff")
            .arg("--name-only")
            .hidden_args(["--no-ext-diff", "--ignore-submodules"])
            .arg("HEAD");

        assert_eq!(cmd.to_string(), "git diff --name-only HEAD");
        assert_eq!(
            command_log_string(&cmd),
            "git -c core.useBuiltinFSMonitor=false diff --name-only --no-ext-diff --ignore-submodules HEAD"
        );
    }

    #[test]
    fn command_log_skips_repeated_program_arg() {
        let mut cmd = Cmd::new("python");
        cmd.arg("python").arg("-m").arg("module");

        assert_eq!(cmd.to_string(), "python -m module");
        assert_eq!(command_log_string(&cmd), "python -m module");
    }

    #[test]
    fn hidden_args_merges_adjacent_ranges() {
        let mut cmd = Cmd::new("uv");
        cmd.arg("venv")
            .arg("/tmp/python-abc")
            .hidden_args(["--python-preference", "managed"])
            .hidden_args(["--no-project"])
            .hidden_args(["--project", "/"]);

        assert_eq!(cmd.to_string(), "uv venv /tmp/python-abc");
        assert_eq!(cmd.hidden_arg_ranges, vec![2..7]);
        assert_eq!(cmd.file_arg_boundary, usize::MAX);
    }

    #[tokio::test]
    async fn output_with_sink_streams_piped_stdout_and_stderr() {
        let chunks = Arc::new(Mutex::new(0));
        let output = Cmd::new("/bin/sh")
            .arg("-c")
            .arg("printf 'OUT\\n'; printf 'ERR\\n' >&2")
            .check(false)
            .output_with_sink(RecordingSink {
                chunks: Arc::clone(&chunks),
            })
            .await
            .expect("piped command should succeed");

        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stdout.contains("OUT\n"));
        assert!(stderr.contains("ERR\n"));
        assert_ne!(*chunks.lock().unwrap(), 0);
    }

    #[tokio::test]
    async fn pty_output_captures_trailing_output_after_fast_exit() {
        for _ in 0..20 {
            let output = Cmd::new("/bin/sh")
                .arg("-c")
                .arg("printf 'FINAL\\n'")
                .check(false)
                .run_on_pty(RecordingSink::default())
                .await
                .expect("pty command should succeed");

            assert!(output.status.success());
            let stdout = String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n");
            assert_eq!(stdout, "FINAL\n");
            assert!(output.stderr.is_empty());
        }
    }
}
