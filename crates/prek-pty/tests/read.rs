#![cfg(unix)]

use std::future::poll_fn;
use std::mem::MaybeUninit;
use std::pin::Pin;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncRead, ReadBuf};

/// Appending to a partially filled buffer must not mark bytes beyond its capacity as initialized.
#[tokio::test]
async fn read_into_partially_filled_buffer() -> Result<(), Box<dyn std::error::Error>> {
    let (mut pty, pts) = prek_pty::open()?;
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
