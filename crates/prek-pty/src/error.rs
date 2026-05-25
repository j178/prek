/// Error type for errors from this crate
#[derive(Debug)]
pub enum Error {
    /// error came from `std::io::Error`
    Io(std::io::Error),
    /// error came from `nix::Error`
    Rustix(rustix::io::Errno),
    /// unsplit was called on halves of two different ptys
    Unsplit(crate::pty::OwnedReadPty, crate::pty::OwnedWritePty),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "{e}"),
            Self::Rustix(e) => write!(f, "{e}"),
            Self::Unsplit(..) => {
                write!(f, "unsplit called on halves of two different ptys")
            }
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<rustix::io::Errno> for Error {
    fn from(e: rustix::io::Errno) -> Self {
        Self::Rustix(e)
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Rustix(e) => Some(e),
            Self::Unsplit(..) => None,
        }
    }
}

/// Convenience wrapper for `Result`s using [`Error`](Error)
pub type Result<T> = std::result::Result<T, Error>;
