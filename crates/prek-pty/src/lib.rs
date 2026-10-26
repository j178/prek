//! PTY output capture for prek, adapted from `pty-process`.
//!
//! See the crate README for the upstream revision and local changes.

#![cfg(unix)]

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
pub fn open() -> io::Result<(Pty, Pts)> {
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
pub struct Pty(AsyncFd<OwnedFd>);

impl Pty {
    /// Read available bytes without waiting for readiness.
    ///
    /// Returns `WouldBlock` when no output is currently available. This bypasses
    /// cached readiness so callers can drain trailing output after the child exits.
    pub fn try_read(&self, buf: &mut [u8]) -> io::Result<usize> {
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
pub struct Pts(OwnedFd);

impl Pts {
    /// Consume the child end to create stdout and stderr handles.
    ///
    /// Callers should give the child null stdin and avoid making this its
    /// controlling terminal. Keep a parent copy of the child-side handles until
    /// output capture completes: macOS discards unread output when the last one closes.
    pub fn into_stdio(self) -> io::Result<(Stdio, Stdio)> {
        Ok((self.0.try_clone()?.into(), self.0.into()))
    }
}
