//! Background pump that drains a spawned child's stderr into a per-run file
//! (and optionally mirrors it live to the parent's stderr).
//!
//! Split out of `stdio.rs` so that file stays under the 300-line production
//! convention (stdio.rs was already 189 lines before Feature 2). Declared as a
//! private child module of `stdio` via `#[path = "stderr_pump.rs"]` so that
//! `transport/mod.rs` (owned by another agent) doesn't need a new `pub mod`
//! line — see ADR 0013.
//!
//! Lifecycle (mirrors the `sse`/`ws` reader-task pattern): the task is owned by
//! a `JoinHandle` stored on `StdioTransport`, and a `CancellationToken` lets
//! graceful `shutdown` first waits for it to drain the child's closed pipe
//! through EOF, while a bounded cancellation path / `Drop` stops it when that
//! drain cannot complete. Child-pipe reads and capture-file writes/flushes are
//! gating: their [`std::io::Error`] travels through the task's `JoinHandle` so
//! shutdown cannot claim success with incomplete evidence. Mirroring to the
//! parent's stderr remains best-effort because that stream may be independently
//! closed or redirected; the capture file is the authoritative artifact.

use std::io;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader, BufWriter};
use tokio::process::ChildStderr;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// Read and write buffer size. A burst reaches the capture file in blocks
/// instead of one blocking-pool round trip per line, so a server that logs
/// heavily is not throttled by its own stderr pipe; a throttled server skews
/// the latency being measured and can miss its graceful-exit budget.
const PUMP_BUFFER_BYTES: usize = 64 * 1024;

/// How long a dropped transport lets the cancelled pump flush lines it has
/// already read before aborting it.
const DROP_FLUSH_BUDGET: Duration = Duration::from_secs(2);

/// Stop the pump from a synchronous `Drop`. Cancelling lets its cancel arm
/// flush what is buffered (up to [`PUMP_BUFFER_BYTES`] of lines already read)
/// instead of discarding it; the abort after [`DROP_FLUSH_BUDGET`] keeps a
/// stuck capture write from leaving the task behind.
pub(super) fn stop_on_drop(mut handle: JoinHandle<io::Result<()>>, cancel: &CancellationToken) {
    cancel.cancel();
    match tokio::runtime::Handle::try_current() {
        Ok(runtime) => {
            runtime.spawn(async move {
                if tokio::time::timeout(DROP_FLUSH_BUDGET, &mut handle)
                    .await
                    .is_err()
                {
                    handle.abort();
                }
            });
        }
        Err(_) => handle.abort(),
    }
}

/// Spawn the stderr pump. Reads `stderr` line-by-line, appending each line
/// (newline-terminated) to `file`; when `tee` is true the same line is also
/// written to the parent process's stderr.
///
/// The returned `JoinHandle` is stored on `StdioTransport` (mirrors the
/// `ws`/`sse` reader task): graceful `shutdown` awaits EOF first and only
/// cancels when bounded draining stalls; `Drop` stops it through
/// [`stop_on_drop`] as a backstop. Capture/read failures are returned by the task and gate shutdown.
pub(super) fn spawn_stderr_pump(
    stderr: ChildStderr,
    file: tokio::fs::File,
    tee: bool,
    cancel: CancellationToken,
) -> JoinHandle<io::Result<()>> {
    spawn_pump(stderr, file, tee, cancel)
}

/// Generic task wrapper keeps failure injection platform-independent in tests
/// while the production entry point above remains typed to `ChildStderr` and
/// `tokio::fs::File`.
fn spawn_pump<R, W>(
    stderr: R,
    file: W,
    tee: bool,
    cancel: CancellationToken,
) -> JoinHandle<io::Result<()>>
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    tokio::spawn(pump_stderr(stderr, file, tee, cancel))
}

async fn pump_stderr<R, W>(
    stderr: R,
    file: W,
    tee: bool,
    cancel: CancellationToken,
) -> io::Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut lines = BufReader::with_capacity(PUMP_BUFFER_BYTES, stderr).lines();
    let mut file = BufWriter::with_capacity(PUMP_BUFFER_BYTES, file);
    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                // Cancellation cannot prove the unread pipe is complete, but
                // it must still surface a capture flush failure to shutdown.
                return file.flush().await;
            }
            result = lines.next_line() => match result {
                Ok(Some(line)) => {
                    file.write_all(line.as_bytes()).await?;
                    file.write_all(b"\n").await?;
                    if tee {
                        // Non-gating by policy: the capture file above is the
                        // durable artifact. A closed parent stderr (for example
                        // a detached terminal or downstream pipe) must not turn
                        // a complete capture into a failed load-test teardown.
                        let mut parent_stderr = tokio::io::stderr();
                        let _ = parent_stderr.write_all(line.as_bytes()).await;
                        let _ = parent_stderr.write_all(b"\n").await;
                    }
                    // Flush once everything read so far is written, so the
                    // file stays current while the child is quiet.
                    if lines.get_ref().buffer().is_empty() {
                        file.flush().await?;
                    }
                }
                // EOF: child closed stderr (usually because it exited).
                // Flush is gating; success means the complete tail is durable.
                Ok(None) => return file.flush().await,
                // Preserve the read failure after attempting to flush bytes
                // already captured. If that flush also fails, return one
                // contextual I/O error containing both failures.
                Err(read_error) => {
                    return match file.flush().await {
                        Ok(()) => Err(read_error),
                        Err(flush_error) => Err(io::Error::new(
                            flush_error.kind(),
                            format!(
                                "child stderr read failed ({read_error}); \
                                 capture flush after read failure also failed ({flush_error})"
                            ),
                        )),
                    };
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::pin::Pin;
    use std::sync::{Arc, Mutex};
    use std::task::{Context, Poll, Waker};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};
    use tokio::io::ReadBuf;

    #[derive(Clone, Copy)]
    enum WriterFailure {
        Write,
        Flush,
    }

    /// Platform-independent capture sink whose selected operation fails.
    struct InjectedFailWriter {
        fail_at: WriterFailure,
    }

    impl AsyncWrite for InjectedFailWriter {
        fn poll_write(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<io::Result<usize>> {
            match self.fail_at {
                WriterFailure::Write => {
                    Poll::Ready(Err(io::Error::other("injected capture write failure")))
                }
                WriterFailure::Flush => Poll::Ready(Ok(buf.len())),
            }
        }

        fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            match self.fail_at {
                WriterFailure::Write => Poll::Ready(Ok(())),
                WriterFailure::Flush => {
                    Poll::Ready(Err(io::Error::other("injected capture flush failure")))
                }
            }
        }

        fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    /// Records each write the pump hands to the capture sink.
    #[derive(Clone, Default)]
    struct RecordingWriter {
        writes: Arc<Mutex<Vec<Vec<u8>>>>,
    }

    impl AsyncWrite for RecordingWriter {
        fn poll_write(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<io::Result<usize>> {
            self.writes.lock().unwrap().push(buf.to_vec());
            Poll::Ready(Ok(buf.len()))
        }

        fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }

        fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    struct InjectedFailReader;

    impl AsyncRead for InjectedFailReader {
        fn poll_read(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            _buf: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            Poll::Ready(Err(io::Error::other("injected child stderr read failure")))
        }
    }

    /// Unique temp path; we only need a *path* a child could write — here we
    /// drive the pump's file half directly via a real `ChildStderr` from a
    /// short-lived child process so the EOF→flush path is exercised end-to-end.
    fn temp_log(tag: &str) -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!(
            "mcp-loadtest-pump-{tag}-{}-{nanos}.log",
            std::process::id()
        ))
    }

    /// Spawn a child whose only job is to print two lines to stderr and exit.
    /// Asserts the pump captured both lines and flushed them on EOF. Uses the
    /// platform shell so the test doesn't depend on Python being present.
    #[tokio::test]
    async fn captures_child_stderr_and_flushes_on_eof() {
        let log = temp_log("eof");
        let file = tokio::fs::File::create(&log)
            .await
            .expect("create temp log");

        #[cfg(windows)]
        let mut child = tokio::process::Command::new("cmd")
            .args(["/C", "echo line-one 1>&2 & echo line-two 1>&2"])
            .stderr(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .expect("spawn child");

        #[cfg(not(windows))]
        let mut child = tokio::process::Command::new("sh")
            .args(["-c", "echo line-one 1>&2; echo line-two 1>&2"])
            .stderr(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .expect("spawn child");

        let stderr = child.stderr.take().expect("child stderr piped");
        let cancel = CancellationToken::new();
        let handle = spawn_stderr_pump(stderr, file, false, cancel.clone());

        // Child exits quickly → stderr hits EOF → pump flushes & returns.
        tokio::time::timeout(Duration::from_secs(10), child.wait())
            .await
            .expect("child wait timed out")
            .expect("child wait errored");
        tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .expect("pump did not finish after child EOF")
            .expect("pump task panicked")
            .expect("pump capture/read I/O failed");

        let contents = tokio::fs::read_to_string(&log)
            .await
            .expect("read captured log");
        let _ = tokio::fs::remove_file(&log).await;

        assert!(
            contents.contains("line-one") && contents.contains("line-two"),
            "captured log should contain both stderr lines, got: {contents:?}"
        );
    }

    /// Python interpreter, overridable via `MCP_LOADTEST_PYTHON` (repo
    /// convention — mirrors `tests/helpers::python()`). Used for a child whose
    /// stderr/lifetime we can shape precisely.
    fn python() -> String {
        std::env::var("MCP_LOADTEST_PYTHON").unwrap_or_else(|_| "python".to_string())
    }

    /// Cancelling the token must stop the pump **promptly** even while the
    /// source is still alive (the shutdown / Drop path), with the captured
    /// file left intact.
    ///
    /// The child writes one stderr line, flushes, then sleeps 30s without
    /// emitting more — so the pump stays genuinely blocked in `next_line()`
    /// (not at EOF) the whole time. Rather than a fixed sleep before
    /// cancelling (racy: on a slow/loaded CI runner the child + interpreter
    /// may not have emitted the line inside a fixed window, leaving the file
    /// empty at cancel — the original macOS/Windows CI flake), we **poll the
    /// file until the line is observed**. `tokio::fs::File` is unbuffered, so
    /// the line is visible the instant the pump's read arm writes it. Only
    /// then do we cancel and assert the `JoinHandle` resolves far inside the
    /// child's 30s lifetime (a broken cancel would block ~30s). The child is
    /// reaped explicitly (`start_kill` + bounded `wait`) so the runtime never
    /// blocks on its sleep at teardown (that original bug made this ~59s).
    #[tokio::test]
    async fn cancel_stops_pump_while_child_alive() {
        let log = temp_log("cancel");
        let file = tokio::fs::File::create(&log)
            .await
            .expect("create temp log");

        // Stdlib-only: emit one stderr line + flush, then stay alive (no more
        // output) for 30s. Cross-platform via the resolved interpreter.
        let mut child = tokio::process::Command::new(python())
            .args([
                "-c",
                "import sys,time; sys.stderr.write('early\\n'); \
                 sys.stderr.flush(); time.sleep(30)",
            ])
            .stderr(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .expect("spawn child");

        let stderr = child.stderr.take().expect("child stderr piped");
        let cancel = CancellationToken::new();
        let handle = spawn_stderr_pump(stderr, file, false, cancel.clone());

        // Deterministically wait until the pump has actually read+written the
        // line (child still sleeping → no EOF; this proves the alive-source
        // read path). Generous bound tolerates slow-CI process startup;
        // normally completes in well under 100ms.
        let observed = {
            let deadline = std::time::Instant::now() + Duration::from_secs(30);
            loop {
                if tokio::fs::read_to_string(&log)
                    .await
                    .map(|c| c.contains("early"))
                    .unwrap_or(false)
                {
                    break true;
                }
                if std::time::Instant::now() >= deadline {
                    break false;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        };
        assert!(
            observed,
            "pump never wrote the child's stderr line to the file \
             (real pump bug, not a timing flake)"
        );

        // Source is still alive (child sleeping 30s). Cancelling must stop the
        // pump promptly: a broken cancel would leave the handle pending until
        // the child's 30s sleep ends, so 5s is unambiguous yet CI-tolerant.
        cancel.cancel();
        tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .expect("cancel must stop the pump within 5s (source still alive)")
            .expect("pump task panicked")
            .expect("pump capture/read I/O failed");

        // The captured line survived the cancel/flush exit path.
        let contents = tokio::fs::read_to_string(&log)
            .await
            .expect("read captured log");
        assert!(
            contents.contains("early"),
            "cancel path must preserve the captured line, got: {contents:?}"
        );

        // Reap the still-sleeping child now (don't leave it for the runtime to
        // block on at drop — that was the original 59s bug). `start_kill` is
        // async-signal-only; the bounded `wait` actually collects it.
        let _ = child.start_kill();
        tokio::time::timeout(Duration::from_secs(2), child.wait())
            .await
            .expect("child reap timed out")
            .expect("child reap errored");
        let _ = tokio::fs::remove_file(&log).await;
    }

    #[tokio::test]
    async fn capture_write_failure_propagates_through_join_handle() {
        let (mut source, stderr) = tokio::io::duplex(64);
        source
            .write_all(b"captured-line\n")
            .await
            .expect("seed injected stderr");
        drop(source);

        let handle = spawn_pump(
            stderr,
            InjectedFailWriter {
                fail_at: WriterFailure::Write,
            },
            false,
            CancellationToken::new(),
        );
        let error = tokio::time::timeout(Duration::from_secs(2), handle)
            .await
            .expect("injected writer pump timed out")
            .expect("injected writer pump task panicked")
            .expect_err("capture write failure must gate the pump");
        assert!(
            error.to_string().contains("injected capture write failure"),
            "unexpected pump error: {error}"
        );
    }

    #[tokio::test]
    async fn capture_flush_failure_propagates_through_join_handle() {
        let (source, stderr) = tokio::io::duplex(1);
        drop(source);

        let handle = spawn_pump(
            stderr,
            InjectedFailWriter {
                fail_at: WriterFailure::Flush,
            },
            false,
            CancellationToken::new(),
        );
        let error = tokio::time::timeout(Duration::from_secs(2), handle)
            .await
            .expect("injected flush pump timed out")
            .expect("injected flush pump task panicked")
            .expect_err("capture flush failure must gate the pump");
        assert!(
            error.to_string().contains("injected capture flush failure"),
            "unexpected pump error: {error}"
        );
    }

    #[tokio::test]
    async fn child_stderr_read_failure_propagates_through_join_handle() {
        let handle = spawn_pump(
            InjectedFailReader,
            tokio::io::sink(),
            false,
            CancellationToken::new(),
        );
        let error = tokio::time::timeout(Duration::from_secs(2), handle)
            .await
            .expect("injected reader pump timed out")
            .expect("injected reader pump task panicked")
            .expect_err("child stderr read failure must gate the pump");
        assert!(
            error
                .to_string()
                .contains("injected child stderr read failure"),
            "unexpected pump error: {error}"
        );
    }

    /// A burst already queued in the pipe reaches the sink in a few block
    /// writes, not two per line; per-line writes to a `tokio::fs::File` are
    /// slow enough to stall a chatty child on its stderr pipe.
    #[tokio::test]
    async fn queued_burst_is_written_in_blocks() {
        const LINES: usize = 1000;
        let (mut source, stderr) = tokio::io::duplex(PUMP_BUFFER_BYTES);
        let mut burst = String::new();
        for i in 0..LINES {
            burst.push_str(&format!("line-{i}\n"));
        }
        source
            .write_all(burst.as_bytes())
            .await
            .expect("seed injected stderr");
        drop(source);

        let sink = RecordingWriter::default();
        let handle = spawn_pump(stderr, sink.clone(), false, CancellationToken::new());
        tokio::time::timeout(Duration::from_secs(2), handle)
            .await
            .expect("burst pump timed out")
            .expect("burst pump task panicked")
            .expect("burst pump I/O failed");

        let writes = sink.writes.lock().unwrap();
        assert!(
            writes.len() <= 2,
            "a {}-byte burst took {} sink writes",
            burst.len(),
            writes.len()
        );
        assert_eq!(writes.concat(), burst.into_bytes());
    }

    /// Holds every write pending until opened, recording that one was tried.
    #[derive(Clone, Default)]
    struct GatedWriter {
        state: Arc<Mutex<Gate>>,
    }

    #[derive(Default)]
    struct Gate {
        open: bool,
        attempted: bool,
        waker: Option<Waker>,
        written: Vec<u8>,
    }

    impl GatedWriter {
        fn attempted(&self) -> bool {
            self.state.lock().unwrap().attempted
        }

        fn open(&self) {
            let mut gate = self.state.lock().unwrap();
            gate.open = true;
            if let Some(waker) = gate.waker.take() {
                waker.wake();
            }
        }
    }

    impl AsyncWrite for GatedWriter {
        fn poll_write(
            self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<io::Result<usize>> {
            let mut gate = self.state.lock().unwrap();
            gate.attempted = true;
            if !gate.open {
                gate.waker = Some(cx.waker().clone());
                return Poll::Pending;
            }
            gate.written.extend_from_slice(buf);
            Poll::Ready(Ok(buf.len()))
        }

        fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }

        fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    /// Dropping the transport mid-burst must not discard lines the pump has
    /// already read: the write in flight and the buffered remainder both
    /// reach the capture file.
    #[tokio::test]
    async fn drop_flushes_lines_already_read() {
        let (mut source, stderr) = tokio::io::duplex(4 * PUMP_BUFFER_BYTES);
        let mut burst = String::new();
        for i in 0..3000 {
            burst.push_str(&format!("line-{i:05}-{}\n", "x".repeat(40)));
        }
        source
            .write_all(burst.as_bytes())
            .await
            .expect("seed injected stderr");

        let sink = GatedWriter::default();
        let cancel = CancellationToken::new();
        let handle = spawn_pump(stderr, sink.clone(), false, cancel.clone());
        tokio::time::timeout(Duration::from_secs(2), async {
            while !sink.attempted() {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .expect("pump never wrote to the sink");

        stop_on_drop(handle, &cancel);
        sink.open();
        tokio::time::sleep(Duration::from_millis(200)).await;

        let written = String::from_utf8(sink.state.lock().unwrap().written.clone())
            .expect("captured stderr is UTF-8");
        assert!(
            written.len() >= PUMP_BUFFER_BYTES,
            "only {} bytes reached the sink after drop",
            written.len()
        );
        assert!(written.ends_with('\n'), "drop left a partial line");
        assert!(
            burst.starts_with(&written),
            "drop reordered or invented lines"
        );
        drop(source);
    }
}
