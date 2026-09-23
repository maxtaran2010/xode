use anyhow::{anyhow, Result};
use futures::StreamExt;
use tokio_util::sync::CancellationToken;

/// Minimal SSE reader: calls `on(event, data)` for each event. Returns when stream ends or cancelled.
pub async fn read_sse(
    resp: reqwest::Response,
    cancel: &CancellationToken,
    mut on: impl FnMut(&str, &str) -> Result<bool>,
) -> Result<()> {
    let mut stream = resp.bytes_stream();
    let mut buf: Vec<u8> = Vec::new();
    let mut event = String::new();
    let mut data = String::new();
    loop {
        let chunk = tokio::select! {
            _ = cancel.cancelled() => return Err(anyhow!("cancelled")),
            c = stream.next() => c,
        };
        let Some(chunk) = chunk else { break };
        let chunk = chunk?;
        buf.extend_from_slice(&chunk);
        while let Some(pos) = buf.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = buf.drain(..=pos).collect();
            let line = String::from_utf8_lossy(&line);
            let line = line.trim_end_matches(['\n', '\r']);
            if line.is_empty() {
                if !data.is_empty() {
                    if !on(&event, &data)? {
                        return Ok(());
                    }
                }
                event.clear();
                data.clear();
                continue;
            }
            if let Some(v) = line.strip_prefix("data:") {
                if !data.is_empty() {
                    data.push('\n');
                }
                data.push_str(v.strip_prefix(' ').unwrap_or(v));
            } else if let Some(v) = line.strip_prefix("event:") {
                event = v.trim().to_string();
            } else if line.starts_with('{') && data.is_empty() {
                // Some servers stream bare JSON lines (NDJSON).
                if !on("", line)? {
                    return Ok(());
                }
            }
        }
    }
    if !data.is_empty() {
        on(&event, &data)?;
    }
    Ok(())
}
