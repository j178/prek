# prek-pty

Unix PTY output capture for prek, allowing hooks to emit colors and progress output.

Based on [pty-process 0.5.4](https://github.com/doy/pty-process/tree/ef371a12351784c852ab182c1e69533b09205889),
with the upstream MIT [license](LICENSE) preserved.

## Local changes

- Keeps PTY allocation and Tokio `AsyncRead`, with only readable interest registered.
- Adds `Pty::try_read()` to drain available output after child exit without waiting
  for another readiness notification.
- Provides `Pts::into_stdio()` for stdout and stderr, without allocating a stdin handle.
- Removes writes, split handles, resizing, raw descriptor conversions, session
  setup, command wrappers, and the blocking API.
- Uses `OwnedFd`, direct rustix calls, and `io::Result` in one module. Dependencies
  are limited to rustix and Tokio; `fs-err` and rustix's `termios` feature are unused.

Prek manages spawning, child exit, Linux `EIO` handling, and output sanitization.
Stdin is null; stdout and stderr share the PTY. No session or controlling terminal
is created, and terminal capability queries are not answered.
