// MIT License
// Copyright (c) 2025 Astral Software Inc.
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:

// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.

// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.

//! Helper for adjusting Unix resource limits.
//!
//! Linux has a historically low default limit of 1024 open file descriptors per process.
//! macOS also defaults to a low soft limit (typically 256), though its hard limit is much
//! higher. On modern multi-core machines, these low defaults can cause "too many open files"
//! errors because uv infers concurrency limits from CPU count and may schedule more concurrent
//! work than the default file descriptor limit allows.
//!
//! This module attempts to raise the soft limit to the hard limit at startup to avoid these
//! errors without requiring users to manually configure their shell's `ulimit` settings.
//! The raised limit is inherited by child processes, which is important for commands like
//! `uv run` that spawn Python interpreters.
//!
//! See: <https://github.com/astral-sh/uv/issues/16999>

use rustix::io::Errno;
use rustix::process::{Resource, Rlimit, getrlimit, setrlimit};
use thiserror::Error;

/// Errors that can occur when adjusting resource limits.
#[derive(Debug, Error)]
pub enum OpenFileLimitError {
    #[error("Soft limit ({current:?}) already meets the target ({target})")]
    AlreadySufficient { current: Option<u64>, target: u64 },

    #[error("Failed to raise open file limit from {current:?} to {target}: {source}")]
    SetLimitFailed {
        current: Option<u64>,
        target: u64,
        source: Errno,
    },
}

/// Maximum file descriptor limit to request.
///
/// We cap at 0x100000 (1,048,576) to match the typical Linux default (`/proc/sys/fs/nr_open`)
/// and to avoid issues with extremely high limits.
///
/// `OpenJDK` uses this same cap because:
///
/// 1. Some code breaks if `RLIMIT_NOFILE` exceeds `i32::MAX` (despite the type being `u64`)
/// 2. Code that iterates over all possible FDs, e.g., to close them, can timeout
///
/// See: <https://bugs.openjdk.org/browse/JDK-8324577>
/// See: <https://github.com/oracle/graal/issues/11136>
///
const MAX_NOFILE_LIMIT: u64 = 0x0010_0000;

/// Attempt to raise the open file descriptor limit to the maximum allowed.
///
/// This function tries to set the soft limit to `min(hard_limit, 0x100000)`. If the operation
/// fails, it returns an error since the default limits may still be sufficient for the
/// current workload.
///
/// Returns [`Ok`] with the new soft limit on successful adjustment, or an appropriate
/// [`OpenFileLimitError`] if adjustment failed.
///
/// Note that `rustix::process::Rlimit` represents unlimited values as `None`.
pub fn adjust_open_file_limit() -> Result<u64, OpenFileLimitError> {
    let rlimit = getrlimit(Resource::Nofile);

    let soft = rlimit.current;
    let hard = rlimit.maximum;

    // Cap the target limit to avoid issues with extremely high values.
    // If hard is unlimited, use MAX_NOFILE_LIMIT.
    let target = hard.unwrap_or(MAX_NOFILE_LIMIT).min(MAX_NOFILE_LIMIT);

    if soft.is_none() || soft.is_some_and(|soft| soft >= target) {
        return Err(OpenFileLimitError::AlreadySufficient {
            current: soft,
            target,
        });
    }

    // Try to raise the soft limit to the target.
    setrlimit(
        Resource::Nofile,
        Rlimit {
            current: Some(target),
            maximum: hard,
        },
    )
    .map_err(|err| OpenFileLimitError::SetLimitFailed {
        current: soft,
        target,
        source: err,
    })?;

    Ok(target)
}
