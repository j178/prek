// Vendored crate from: https://crates.io/crates/pty-process

mod error;
#[allow(clippy::module_inception)]
mod pty;
mod sys;
mod types;

pub(crate) use error::{Error, Result};
#[allow(unused_imports)]
pub use pty::{OwnedReadPty, OwnedWritePty, Pts, Pty, ReadPty, WritePty, open};
pub(crate) use types::Size;
