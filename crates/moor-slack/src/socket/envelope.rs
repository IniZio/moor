//! Handles one Socket Mode envelope: hands the item to the intake and acknowledges it.

use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::{json, Value};
use tungstenite::{Message, WebSocket};

use super::parse::{parse_click, parse_mention, parse_slash, parse_submission};
use super::{flush, is_timeout, Inbound, Intake, Kept, NotKept, Roleplay, SocketError, Stream};

/// What one envelope asks the read loop to do next.
pub(super) enum Flow {
    /// Keep reading.
    Continue,
    /// Slack greeted us; the connection is healthy.
    Hello,
    /// Slack asked us to reconnect.
    Disconnect,
    /// The daemon is stopping; the envelope was kept but not acked.
    Stop,
}

/// Hands a mention or click in one JSON envelope to `intake`, then acknowledges the envelope.
/// An item the intake could not keep gets no ack, so Slack sends it again; neither does one
/// that was kept after `stop` was set.
pub(super) fn handle_envelope(
    text: &str,
    ws: &mut WebSocket<Stream>,
    intake: &dyn Intake,
    roleplay: Option<&Roleplay>,
    stop: &AtomicBool,
) -> Result<Flow, SocketError> {
    let Ok(envelope) = serde_json::from_str::<Value>(text) else {
        return Ok(Flow::Continue);
    };
    let (flow, item) = match envelope.get("type").and_then(Value::as_str) {
        Some("hello") => (Flow::Hello, None),
        Some("disconnect") => (Flow::Disconnect, None),
        Some("events_api") => (
            Flow::Continue,
            parse_mention(&envelope, roleplay).map(Inbound::Mention),
        ),
        Some("interactive") => (
            Flow::Continue,
            parse_click(&envelope)
                .map(Inbound::Click)
                .or_else(|| parse_submission(&envelope).map(Inbound::Submission)),
        ),
        Some("slash_commands") => (Flow::Continue, parse_slash(&envelope).map(Inbound::Slash)),
        _ => (Flow::Continue, None),
    };
    match item.map_or_else(|| Ok(Kept::default()), |item| intake.take(item)) {
        Ok(_) if stop.load(Ordering::SeqCst) => Ok(Flow::Stop),
        Ok(kept) => {
            ack(ws, &envelope, kept.ack)?;
            if let Some(then) = kept.then {
                flush(ws)?;
                intake.after_ack(then);
            }
            Ok(flow)
        }
        Err(NotKept::Failed) => Ok(flow),
        Err(NotKept::Gone) => Err(SocketError::ReceiverGone),
    }
}

/// Sends the ack for `envelope`, with `payload` when there is one, when it carries an
/// envelope id.
fn ack(
    ws: &mut WebSocket<Stream>,
    envelope: &Value,
    payload: Option<Value>,
) -> Result<(), SocketError> {
    let Some(id) = envelope.get("envelope_id").and_then(Value::as_str) else {
        return Ok(());
    };
    let frame = payload.map_or_else(
        || json!({"envelope_id": id}),
        |payload| json!({"envelope_id": id, "payload": payload}),
    );
    match ws.send(Message::text(frame.to_string())) {
        Ok(()) => Ok(()),
        Err(tungstenite::Error::Io(e)) if is_timeout(&e) => Ok(()),
        Err(e) => Err(SocketError::Io(e.to_string())),
    }
}
