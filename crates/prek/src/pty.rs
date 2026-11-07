// This software is Copyright (c) 2021 by Jesse Luehrs.
//
// This is free software, licensed under:
//
//   The MIT (X11) License
//
// The MIT License
//
// Permission is hereby granted, free of charge, to any person
// obtaining a copy of this software and associated
// documentation files (the "Software"), to deal in the Software
// without restriction, including without limitation the rights to
// use, copy, modify, merge, publish, distribute, sublicense,
// and/or sell copies of the Software, and to permit persons to
// whom the Software is furnished to do so, subject to the
// following conditions:
//
// The above copyright notice and this permission notice shall
// be included in all copies or substantial portions of the
// Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT
// WARRANTY OF ANY KIND, EXPRESS OR IMPLIED,
// INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF
// MERCHANTABILITY, FITNESS FOR A PARTICULAR
// PURPOSE AND NONINFRINGEMENT. IN NO EVENT
// SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE
// LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT,
// TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN
// CONNECTION WITH THE SOFTWARE OR THE USE OR
// OTHER DEALINGS IN THE SOFTWARE.

//! Unix PTY output capture for hook colors and progress output.
//!
//! Based on [pty-process 0.5.4](https://github.com/doy/pty-process/tree/ef371a12351784c852ab182c1e69533b09205889).

use std::io;
use std::os::fd::OwnedFd;
use std::pin::Pin;
use std::process::Stdio;
use std::task::{Context, Poll, ready};

use rustix::fs::{Mode, OFlags};
use rustix::io::FdFlags;
use rustix::pty::OpenptFlags;
use tokio::io::unix::AsyncFd;
use tokio::io::{AsyncRead, Interest, ReadBuf};

/// Allocate a nonblocking PTY reader and the child end of the terminal.
///
/// Must be called within a Tokio runtime with I/O enabled. Neither end becomes
/// the controlling terminal of the calling process.
pub(crate) fn open() -> io::Result<(Pty, Pts)> {
    let pty = rustix::pty::openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY)?;
    rustix::pty::grantpt(&pty)?;
    rustix::pty::unlockpt(&pty)?;

    // POSIX openpt does not support CLOEXEC on all supported platforms.
    let flags = rustix::io::fcntl_getfd(&pty)?;
    rustix::io::fcntl_setfd(&pty, flags | FdFlags::CLOEXEC)?;

    // Open the child end before registering the master, as required on macOS.
    let pts = rustix::fs::open(
        rustix::pty::ptsname(&pty, Vec::new())?,
        OFlags::RDWR | OFlags::NOCTTY | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    let flags = rustix::fs::fcntl_getfl(&pty)?;
    rustix::fs::fcntl_setfl(&pty, flags | OFlags::NONBLOCK)?;

    // SAFETY: OwnedFd keeps the descriptor open, and Pty never exposes or replaces it.
    let pty = unsafe { AsyncFd::register_with_interest(pty, Interest::READABLE) }?;
    Ok((Pty(pty), Pts(pts)))
}

/// The parent end of a PTY, used to capture the child's output.
pub(crate) struct Pty(AsyncFd<OwnedFd>);

impl Pty {
    /// Read available bytes without waiting for readiness.
    ///
    /// Returns `WouldBlock` when no output is currently available. This bypasses
    /// cached readiness so callers can drain trailing output after the child exits.
    pub(crate) fn try_read(&self, buf: &mut [u8]) -> io::Result<usize> {
        Ok(rustix::io::read(self.0.get_ref(), buf)?)
    }
}

impl AsyncRead for Pty {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if buf.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }

        loop {
            let mut guard = ready!(self.0.poll_read_ready(cx))?;
            // SAFETY: rustix::io::read only initializes bytes in the supplied buffer.
            let unfilled = unsafe { buf.unfilled_mut() };
            match guard
                .try_io(|inner| rustix::io::read(inner.get_ref(), unfilled).map_err(Into::into))
            {
                Ok(Ok((filled, _))) => {
                    let bytes = filled.len();
                    // SAFETY: read initialized exactly these bytes after the filled region.
                    // assume_init counts from that region's end, not the buffer's start.
                    unsafe { buf.assume_init(bytes) };
                    buf.advance(bytes);
                    return Poll::Ready(Ok(()));
                }
                Ok(Err(err)) => return Poll::Ready(Err(err)),
                Err(_would_block) => {}
            }
        }
    }
}

/// The child end of a PTY, intended for stdout and stderr only.
pub(crate) struct Pts(OwnedFd);

impl Pts {
    /// Consume the child end to create stdout and stderr handles.
    ///
    /// Callers should give the child null stdin and avoid making this its
    /// controlling terminal. Keep a parent copy of the child-side handles until
    /// output capture completes: macOS discards unread output when the last one closes.
    pub(crate) fn into_stdio(self) -> io::Result<(Stdio, Stdio)> {
        Ok((self.0.try_clone()?.into(), self.0.into()))
    }
}

#[cfg(test)]
mod tests {
    use std::future::poll_fn;
    use std::mem::MaybeUninit;
    use std::pin::Pin;
    use std::process::Stdio;
    use std::time::Duration;

    use tokio::io::{AsyncRead, ReadBuf};

    use super::open;

    /// Appending to a partially filled buffer must not mark bytes beyond its capacity as initialized.
    #[tokio::test]
    async fn read_into_partially_filled_buffer() -> Result<(), Box<dyn std::error::Error>> {
        let (mut pty, pts) = open()?;
        let (stdout, stderr) = pts.into_stdio()?;
        // Keep Command's slave handles alive until the output has been read.
        let mut command = tokio::process::Command::new("/bin/sh");
        let mut child = command
            .args(["-c", "printf output"])
            .stdin(Stdio::null())
            .stdout(stdout)
            .stderr(stderr)
            .kill_on_drop(true)
            .spawn()?;

        let mut storage = [MaybeUninit::uninit(); 12];
        let mut buf = ReadBuf::uninit(&mut storage);
        buf.put_slice(b"prefix");
        while buf.remaining() > 0 {
            let previous = buf.filled().len();
            tokio::time::timeout(
                Duration::from_secs(10),
                poll_fn(|cx| Pin::new(&mut pty).poll_read(cx, &mut buf)),
            )
            .await??;
            assert!(buf.filled().len() > previous, "unexpected EOF");
        }

        assert_eq!(buf.filled(), b"prefixoutput");
        assert_eq!(buf.initialized().len(), 12);
        assert!(child.wait().await?.success());
        Ok(())
    }
}
