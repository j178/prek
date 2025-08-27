#![allow(clippy::module_name_repetitions)]
use std::io::Write as _;

type AsyncPty = tokio::io::unix::AsyncFd<crate::sys::Pty>;

/// Allocate and return a new pty and pts.
///
/// # Errors
/// Returns an error if the pty failed to be allocated, or if we were
/// unable to put it into non-blocking mode.
pub fn open() -> crate::Result<(Pty, Pts)> {
    let pty = crate::sys::Pty::open()?;
    let pts = pty.pts()?;
    pty.set_nonblocking()?;
    let pty = tokio::io::unix::AsyncFd::new(pty)?;
    Ok((Pty(pty), Pts(pts)))
}

/// An allocated pty
pub struct Pty(AsyncPty);

impl Pty {
    /// Use the provided file descriptor as a pty.
    ///
    /// # Safety
    /// The provided file descriptor must be valid, open, belong to a pty,
    /// and put into nonblocking mode.
    ///
    /// # Errors
    /// Returns an error if it fails to be registered with the async runtime.
    pub unsafe fn from_fd(fd: std::os::fd::OwnedFd) -> crate::Result<Self> {
        Ok(Self(tokio::io::unix::AsyncFd::new(unsafe {
            crate::sys::Pty::from_fd(fd)
        })?))
    }

    /// Change the terminal size associated with the pty.
    ///
    /// # Errors
    /// Returns an error if we were unable to set the terminal size.
    pub fn resize(&self, size: crate::Size) -> crate::Result<()> {
        self.0.get_ref().set_term_size(size)
    }

    /// Splits a `Pty` into a read half and a write half, which can be used to
    /// read from and write to the pty concurrently. Does not allocate, but
    /// the returned halves cannot be moved to independent tasks.
    pub fn split(&self) -> (ReadPty<'_>, WritePty<'_>) {
        (ReadPty(&self.0), WritePty(&self.0))
    }

    /// Splits a `Pty` into a read half and a write half, which can be used to
    /// read from and write to the pty concurrently. This method requires an
    /// allocation, but the returned halves can be moved to independent tasks.
    /// The original `Pty` instance can be recovered via the
    /// [`OwnedReadPty::unsplit`] method.
    #[must_use]
    pub fn into_split(self) -> (OwnedReadPty, OwnedWritePty) {
        let Self(pt) = self;
        let read_pt = std::sync::Arc::new(pt);
        let write_pt = std::sync::Arc::clone(&read_pt);
        (OwnedReadPty(read_pt), OwnedWritePty(write_pt))
    }
}

impl From<Pty> for std::os::fd::OwnedFd {
    fn from(pty: Pty) -> Self {
        pty.0.into_inner().into()
    }
}

impl std::os::fd::AsFd for Pty {
    fn as_fd(&self) -> std::os::fd::BorrowedFd<'_> {
        self.0.get_ref().as_fd()
    }
}

impl std::os::fd::AsRawFd for Pty {
    fn as_raw_fd(&self) -> std::os::fd::RawFd {
        self.0.get_ref().as_raw_fd()
    }
}

impl tokio::io::AsyncRead for Pty {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf,
    ) -> std::task::Poll<std::io::Result<()>> {
        poll_read(&self.0, cx, buf)
    }
}

impl tokio::io::AsyncWrite for Pty {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        poll_write(&self.0, cx, buf)
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        poll_flush(&self.0, cx)
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
        std::task::Poll::Ready(Ok(()))
    }
}

/// The child end of the pty
///
/// See [`open`] and [`Command::spawn`](crate::Command::spawn)
pub struct Pts(pub(crate) crate::sys::Pts);

impl Pts {
    /// Use the provided file descriptor as a pts.
    ///
    /// # Safety
    /// The provided file descriptor must be valid, open, and belong to the
    /// child end of a pty.
    #[must_use]
    pub unsafe fn from_fd(fd: std::os::fd::OwnedFd) -> Self {
        Self(unsafe { crate::sys::Pts::from_fd(fd) })
    }

    pub fn setup_subprocess(
        &self,
    ) -> std::io::Result<(
        std::process::Stdio,
        std::process::Stdio,
        std::process::Stdio,
    )> {
        self.0.setup_subprocess()
    }

    pub fn session_leader(&self) -> impl FnMut() -> std::io::Result<()> + use<> {
        self.0.session_leader()
    }
}

impl std::os::fd::AsFd for Pts {
    fn as_fd(&self) -> std::os::fd::BorrowedFd<'_> {
        self.0.as_fd()
    }
}

impl std::os::fd::AsRawFd for Pts {
    fn as_raw_fd(&self) -> std::os::fd::RawFd {
        self.0.as_raw_fd()
    }
}

/// Borrowed read half of a [`Pty`]
pub struct ReadPty<'a>(&'a AsyncPty);

impl tokio::io::AsyncRead for ReadPty<'_> {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf,
    ) -> std::task::Poll<std::io::Result<()>> {
        poll_read(self.0, cx, buf)
    }
}

/// Borrowed write half of a [`Pty`]
pub struct WritePty<'a>(&'a AsyncPty);

impl WritePty<'_> {
    /// Change the terminal size associated with the pty.
    ///
    /// # Errors
    /// Returns an error if we were unable to set the terminal size.
    pub fn resize(&self, size: crate::Size) -> crate::Result<()> {
        self.0.get_ref().set_term_size(size)
    }
}

impl tokio::io::AsyncWrite for WritePty<'_> {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        poll_write(self.0, cx, buf)
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        poll_flush(self.0, cx)
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
        std::task::Poll::Ready(Ok(()))
    }
}

/// Owned read half of a [`Pty`]
#[derive(Debug)]
pub struct OwnedReadPty(std::sync::Arc<AsyncPty>);

impl OwnedReadPty {
    /// Attempt to join the two halves of a `Pty` back into a single instance.
    /// The two halves must have originated from calling
    /// [`into_split`](Pty::into_split) on a single instance.
    ///
    /// # Errors
    /// Returns an error if the two halves came from different [`Pty`]
    /// instances. The mismatched halves are returned as part of the error.
    pub fn unsplit(self, write_half: OwnedWritePty) -> crate::Result<Pty> {
        let Self(read_pt) = self;
        let OwnedWritePty(write_pt) = write_half;
        if std::sync::Arc::ptr_eq(&read_pt, &write_pt) {
            drop(write_pt);
            Ok(Pty(std::sync::Arc::try_unwrap(read_pt)
                // it shouldn't be possible for more than two references to
                // the same pty to exist
                .unwrap_or_else(|_| unreachable!())))
        } else {
            Err(crate::Error::Unsplit(
                Self(read_pt),
                OwnedWritePty(write_pt),
            ))
        }
    }
}

impl tokio::io::AsyncRead for OwnedReadPty {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf,
    ) -> std::task::Poll<std::io::Result<()>> {
        poll_read(&self.0, cx, buf)
    }
}

/// Owned write half of a [`Pty`]
#[derive(Debug)]
pub struct OwnedWritePty(std::sync::Arc<AsyncPty>);

impl OwnedWritePty {
    /// Change the terminal size associated with the pty.
    ///
    /// # Errors
    /// Returns an error if we were unable to set the terminal size.
    pub fn resize(&self, size: crate::Size) -> crate::Result<()> {
        self.0.get_ref().set_term_size(size)
    }
}

impl tokio::io::AsyncWrite for OwnedWritePty {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        poll_write(&self.0, cx, buf)
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        poll_flush(&self.0, cx)
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
        std::task::Poll::Ready(Ok(()))
    }
}

fn poll_read(
    pty: &AsyncPty,
    cx: &mut std::task::Context<'_>,
    buf: &mut tokio::io::ReadBuf,
) -> std::task::Poll<std::io::Result<()>> {
    loop {
        let mut guard = match pty.poll_read_ready(cx) {
            std::task::Poll::Ready(guard) => guard,
            std::task::Poll::Pending => return std::task::Poll::Pending,
        }?;
        let prev_filled = buf.filled().len();
        // SAFETY: we only pass b to read_buf, which never uninitializes any
        // part of the buffer it is given
        let b = unsafe { buf.unfilled_mut() };
        match guard.try_io(|inner| inner.get_ref().read_buf(b)) {
            Ok(Ok((filled, _unfilled))) => {
                let bytes = filled.len();
                // SAFETY: read_buf is given a buffer that starts at the end
                // of the filled section, and then both initializes and fills
                // some amount of the buffer after that (and never
                // deinitializes anything). we know that at least this many
                // bytes have been initialized (they either were filled and
                // initialized previously, or the call to read_buf did), and
                // assume_init will ignore any attempts to shrink the
                // initialized space, so this call is always safe.
                unsafe { buf.assume_init(prev_filled + bytes) };
                buf.advance(bytes);
                return std::task::Poll::Ready(Ok(()));
            }
            Ok(Err(e)) => return std::task::Poll::Ready(Err(e)),
            Err(_would_block) => {}
        }
    }
}

fn poll_write(
    pty: &AsyncPty,
    cx: &mut std::task::Context<'_>,
    buf: &[u8],
) -> std::task::Poll<std::io::Result<usize>> {
    loop {
        let mut guard = match pty.poll_write_ready(cx) {
            std::task::Poll::Ready(guard) => guard,
            std::task::Poll::Pending => return std::task::Poll::Pending,
        }?;
        match guard.try_io(|inner| inner.get_ref().write(buf)) {
            Ok(result) => return std::task::Poll::Ready(result),
            Err(_would_block) => {}
        }
    }
}

fn poll_flush(
    pty: &AsyncPty,
    cx: &mut std::task::Context<'_>,
) -> std::task::Poll<std::io::Result<()>> {
    loop {
        let mut guard = match pty.poll_write_ready(cx) {
            std::task::Poll::Ready(guard) => guard,
            std::task::Poll::Pending => return std::task::Poll::Pending,
        }?;
        match guard.try_io(|inner| inner.get_ref().flush()) {
            Ok(_) => return std::task::Poll::Ready(Ok(())),
            Err(_would_block) => {}
        }
    }
}
