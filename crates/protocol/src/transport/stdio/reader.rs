//! Bounded line reader for the stdio transport — the OOM guard (L-2).
//!
//! Extracted from `stdio.rs` to keep that file under the 300-line production
//! convention. Declared as a private child module of `stdio` via
//! `#[path = "stdio_line_reader.rs"] mod stdio_line_reader;` inside `stdio.rs`
//! so `transport/mod.rs` (owned by a different workstream) needs no new
//! `pub mod` line — same technique as `stderr_pump`. See ADR 0013.

use tokio::io::AsyncBufReadExt;

use super::MAX_LINE_BYTES;
use crate::transport::TransportError;

/// Read one newline-terminated line from `reader`, appending its raw bytes
/// to `out`, but abort once `MAX_LINE_BYTES` would be exceeded. Returns the
/// number of bytes pushed (including the trailing `\n` if any). Returns `0`
/// on EOF with nothing read.
///
/// This is the OOM guard for L-2: a malicious server-under-test can emit one
/// gigantic line; the default `read_line` would happily buffer it all. We use
/// `fill_buf`/`consume` so we can stop reading the moment we'd cross the cap.
///
/// Bytes, not a `String`: a multi-byte UTF-8 character can straddle two
/// `fill_buf` chunks, so decoding chunk by chunk would corrupt it. The caller
/// decodes the complete line once.
pub(super) async fn read_bounded_line<R>(
    reader: &mut R,
    out: &mut Vec<u8>,
) -> Result<usize, TransportError>
where
    R: tokio::io::AsyncBufRead + Unpin,
{
    let mut total = 0usize;
    loop {
        let buf = reader.fill_buf().await?;
        if buf.is_empty() {
            // EOF.
            return Ok(total);
        }
        let (n, found_newline) = match buf.iter().position(|&b| b == b'\n') {
            Some(pos) => (pos + 1, true),
            None => (buf.len(), false),
        };
        if total + n > MAX_LINE_BYTES {
            return Err(TransportError::Other(format!(
                "stdio transport: response line exceeds {MAX_LINE_BYTES} bytes; aborting to avoid OOM"
            )));
        }
        out.extend_from_slice(&buf[..n]);
        reader.consume(n);
        total += n;
        if found_newline {
            return Ok(total);
        }
    }
}

#[cfg(test)]
mod tests {
    use tokio::io::BufReader;

    use super::*;

    #[tokio::test]
    async fn multibyte_char_split_across_buffer_fills_survives() {
        // "ราคา" is 12 bytes of 3-byte UTF-8 characters. A 4-byte buffer
        // forces every character to straddle a fill_buf boundary.
        let input = "{\"text\":\"ราคา\"}\nnext".as_bytes();
        let mut reader = BufReader::with_capacity(4, input);
        let mut line = Vec::new();

        let n = read_bounded_line(&mut reader, &mut line)
            .await
            .expect("read line");

        assert_eq!(String::from_utf8_lossy(&line), "{\"text\":\"ราคา\"}\n");
        assert_eq!(n, line.len());
    }

    #[tokio::test]
    async fn line_over_the_cap_is_rejected() {
        let input = vec![b'x'; MAX_LINE_BYTES + 1];
        let mut reader = BufReader::new(input.as_slice());
        let mut line = Vec::new();

        let err = read_bounded_line(&mut reader, &mut line)
            .await
            .expect_err("oversized line must be rejected");

        assert!(err.to_string().contains("exceeds"), "{err}");
    }
}
