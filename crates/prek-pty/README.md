# prek-pty

Unix PTY output capture for prek. Hooks see a terminal on stdout and stderr so
they can emit colors and progress output. Stdin remains null, and prek does not
provide an interactive terminal or answer terminal capability queries.

## Upstream

Based on [pty-process 0.5.4](https://github.com/doy/pty-process/tree/ef371a12351784c852ab182c1e69533b09205889),
modified to keep only the functionality prek needs for output capture.
The upstream MIT license is preserved in [LICENSE](LICENSE).

Dependency versions and CI are managed by the prek workspace.

## Retained API

- `open()` allocates a PTY and returns its parent reader and child end.
- `Pty` implements Tokio's `AsyncRead` without pre-initializing the read buffer.
- `Pty::try_read()` reads directly from the nonblocking descriptor, bypassing
  cached readiness so prek can drain trailing bytes after observing child exit.
- `Pts::into_stdio()` consumes the child end and returns stdout and stderr handles
  using one descriptor clone. No unused stdin handle is allocated.

Allocation preserves `NOCTTY` and close-on-exec flags on both ends, opens the child
end before registering the parent end for macOS, and makes only the parent end
nonblocking. Errors are ordinary `std::io::Error` values, including the original
OS error codes. Prek adds allocation and subprocess context at the call site.

## Local changes

- Removed writes, flushing, shutdown, borrowed and owned split halves, unsplitting,
  resizing, pixel dimensions, raw descriptor constructors and conversions, and
  session-leader setup. Omitted the upstream command wrappers and blocking API
  because prek owns process spawning, exit status handling, and output reporting.
- Replaced the separate syscall wrappers and custom error enum with `OwnedFd`,
  rustix calls, and `io::Result` in one module. Opening the child end directly with
  rustix avoids the flag conversion and its `unwrap`. The crate depends only on
  rustix and Tokio, and does not request rustix's `termios` feature.
- Registers only readable interest with Tokio, since prek never writes to the
  parent end of the PTY.
- Preserved normal terminal output processing, including platform newline
  conversion. Reads preserve the bytes delivered by the PTY; output sanitization
  stays in prek.
- Kept subprocess policy in `prek::process`: null stdin, shared PTY stdout/stderr,
  no new session or controlling terminal, and closing the parent's stdio copies
  after each spawn attempt. Making the child a foreground process previously
  caused terminal capability probes to wait for responses that prek cannot send.
- Kept Linux `EIO` handling and exit-time draining in `prek::process`. Draining
  stops on EOF, `EIO`, or `WouldBlock`, so idle descendants that retain a child handle
  do not keep prek waiting after the direct child exits.

## Validation and future updates

`tests/read.rs` checks the upstream partially filled buffer regression without
sleeps or manual polling. Prek's process tests cover trailing output after fast
exit, raw bytes, and output stream setup. Run/output integration snapshots cover
terminal detection, color, and terminal-control sanitization.

For future updates, compare upstream changes from the revision above, port fixes
to allocation and reading, and update this record. Keep process/session policy in
prek and add other upstream APIs only when prek has a concrete caller.
