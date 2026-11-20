//! MCP stdio transport: newline-delimited JSON-RPC on stdin, responses on
//! stdout (one JSON value per line), diagnostics on stderr only.
//!
//! Lines longer than [`MAX_LINE_BYTES`] are discarded without being buffered
//! and answered with a `-32600` error. Requests are handled concurrently
//! (tool calls run on the blocking pool); one writer task serializes output.

use std::io::BufRead;
use std::sync::Arc;

use serde_json::Value;
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;
use tokio::task::JoinSet;

use crate::Shutdown;
use crate::protocol::{Dispatcher, INVALID_REQUEST, error_response};

/// Largest accepted stdin message (bytes).
pub const MAX_LINE_BYTES: usize = 4 * 1024 * 1024;

/// One stdin line, or a line that was too long.
#[derive(Debug, PartialEq, Eq)]
pub enum Line {
    Message(Vec<u8>),
    TooLong,
}

/// Reads one `\n`-terminated line of at most `max` bytes (without the
/// newline). Longer lines are skipped to their end. `None` at EOF.
pub fn read_bounded_line(r: &mut impl BufRead, max: usize) -> std::io::Result<Option<Line>> {
    let mut buf = Vec::new();
    let mut too_long = false;
    let mut any = false;
    loop {
        let chunk = match r.fill_buf() {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        if chunk.is_empty() {
            return Ok(any.then(|| if too_long { Line::TooLong } else { Line::Message(buf) }));
        }
        any = true;
        let (take, done) = match chunk.iter().position(|&b| b == b'\n') {
            Some(i) => (i, true),
            None => (chunk.len(), false),
        };
        if !too_long {
            if buf.len() + take > max {
                too_long = true;
                buf = Vec::new();
            } else {
                buf.extend_from_slice(&chunk[..take]);
            }
        }
        r.consume(if done { take + 1 } else { take });
        if done {
            return Ok(Some(if too_long { Line::TooLong } else { Line::Message(buf) }));
        }
    }
}

/// Serves MCP over stdin/stdout until EOF or `shutdown`.
pub async fn serve_stdio(dispatcher: Arc<Dispatcher>, shutdown: Shutdown) -> anyhow::Result<()> {
    let (in_tx, mut in_rx) = mpsc::channel::<Line>(64);
    // A plain thread: a blocking stdin read must not hold up runtime shutdown.
    std::thread::Builder::new().name("sillage-stdin".into()).spawn(move || {
        let stdin = std::io::stdin();
        let mut lock = stdin.lock();
        loop {
            match read_bounded_line(&mut lock, MAX_LINE_BYTES) {
                Ok(Some(line)) => {
                    if in_tx.blocking_send(line).is_err() {
                        break;
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    eprintln!("sillage: stdin read failed: {e}");
                    break;
                }
            }
        }
    })?;

    let (out_tx, mut out_rx) = mpsc::channel::<Value>(64);
    let writer = tokio::spawn(async move {
        let mut stdout = tokio::io::stdout();
        while let Some(v) = out_rx.recv().await {
            let mut line = v.to_string();
            line.push('\n');
            if stdout.write_all(line.as_bytes()).await.is_err() || stdout.flush().await.is_err() {
                break;
            }
        }
    });

    let mut tasks = JoinSet::new();
    loop {
        let line = tokio::select! {
            _ = shutdown.wait() => break,
            l = in_rx.recv() => match l {
                Some(l) => l,
                None => break,
            },
        };
        let bytes = match line {
            Line::TooLong => {
                let err = error_response(Value::Null, INVALID_REQUEST, format!("message exceeds {MAX_LINE_BYTES} bytes"));
                let _ = out_tx.send(err).await;
                continue;
            }
            Line::Message(b) => b,
        };
        if bytes.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let d = Arc::clone(&dispatcher);
        let tx = out_tx.clone();
        tasks.spawn(async move {
            if let Some(reply) = d.handle_bytes(&bytes).await {
                let _ = tx.send(reply).await;
            }
        });
        while tasks.try_join_next().is_some() {}
    }
    // Finish in-flight requests (EOF) unless shutting down.
    if !shutdown.is_triggered() {
        while tasks.join_next().await.is_some() {}
    } else {
        tasks.abort_all();
    }
    drop(out_tx);
    let _ = writer.await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_lines() {
        let mut r = std::io::BufReader::with_capacity(4, &b"{\"a\":1}\n0123456789\nok\nlast"[..]);
        assert_eq!(read_bounded_line(&mut r, 8).unwrap(), Some(Line::Message(b"{\"a\":1}".to_vec())));
        assert_eq!(read_bounded_line(&mut r, 8).unwrap(), Some(Line::TooLong));
        assert_eq!(read_bounded_line(&mut r, 8).unwrap(), Some(Line::Message(b"ok".to_vec())));
        assert_eq!(read_bounded_line(&mut r, 8).unwrap(), Some(Line::Message(b"last".to_vec())));
        assert_eq!(read_bounded_line(&mut r, 8).unwrap(), None);
    }
}
