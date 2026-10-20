//! Server-Sent Events wire format for relaying controller events to the browser.

use moor_core::controller::ServerFrame;

use moor_client::EventRecord;

/// Comment line sent while the stream is idle.
pub const KEEPALIVE: &[u8] = b": keepalive\n\n";

/// One event as an SSE message: its cursor as the id, the full frame as data.
#[must_use]
pub fn event_frame(event: &EventRecord) -> String {
    let frame = ServerFrame::Event {
        cursor: event.cursor,
        r#box: event.box_name.clone(),
        session: event.session,
        kind: event.kind.clone(),
        payload: event.payload.clone(),
        ts: event.ts,
    };
    let data = serde_json::to_string(&frame).unwrap_or_default();
    format!("id: {}\ndata: {data}\n\n", event.cursor)
}

/// A failure as an SSE `error` message, kept on one data line.
#[must_use]
pub fn error_frame(message: &str) -> String {
    let message = message.replace(['\r', '\n'], " ");
    format!("event: error\ndata: {message}\n\n")
}

/// Cursor to resume from: `Last-Event-ID` if present, else `since`, else 0.
#[must_use]
pub fn start_cursor(last_event_id: Option<&str>, since: Option<&str>) -> i64 {
    last_event_id
        .or(since)
        .and_then(|v| v.trim().parse::<i64>().ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests;
