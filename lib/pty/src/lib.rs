// Vendored crate from: https://crates.io/crates/pty-process

#![cfg(unix)]

mod error;
#[allow(clippy::module_inception)]
mod pty;
mod sys;
mod types;

pub use error::{Error, Result};
pub use pty::{OwnedReadPty, OwnedWritePty, Pts, Pty, ReadPty, WritePty, open};
pub use types::Size;
