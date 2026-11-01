//! The watch loop: list the Boxes, listen on them, read each one that changed and say what
//! happened, and start over when the stream drops.

use std::collections::HashSet;
use std::io;
use std::thread::sleep;
use std::time::Duration;

use serde_json::{json, Value};

use crate::gate::{Fail, Gate, Stream};

/// Every Box resource's URI starts with this.
const PREFIX: &str = "moor://box/";

/// Events worth a line; the rest (busy, ready, asleep) are noise to a reader.
const WORTH: [&str; 8] = [
    "idled",
    "needs_input",
    "setup_failed",
    "session_failed",
    "launch_timed_out",
    "box_failed",
    "relaunch_capped",
    "closed",
];

/// Longest the line's agent text runs.
const SAID: usize = 200;

/// First wait after a lost stream.
const FIRST: Duration = Duration::from_secs(2);

/// Longest wait after a lost stream, and the wait after a rejected token.
const LONGEST: Duration = Duration::from_secs(60);

/// What a stream held that makes the loop go round again.
enum Ended {
    /// The list of Boxes changed; listen again at once.
    Relist,
    /// The stream is gone.
    Lost(Fail),
}

/// What carries over between connections.
struct State {
    /// Events already said, as `uri#id`.
    said: HashSet<String>,
    /// Whether the first connection has not yet listed the Boxes.
    first: bool,
    /// Whether the outage is known to the reader.
    down: bool,
}

/// Runs until `say` fails, which is when nobody reads the lines any more.
pub(crate) fn run(gate: &Gate, say: &mut dyn FnMut(&str) -> io::Result<()>) -> io::Result<()> {
    let mut state = State {
        said: HashSet::new(),
        first: true,
        down: false,
    };
    let mut wait = FIRST;
    loop {
        let lost = match session(gate, &mut state, say)? {
            Ended::Relist => continue,
            Ended::Lost(lost) => lost,
        };
        let pause = match lost {
            Fail::Rejected => {
                if !state.down {
                    say("moor-listen: token rejected; call any moor tool so the client refreshes it")?;
                }
                LONGEST
            }
            Fail::Other(why) => {
                if !state.down {
                    say(&format!("moor-listen: stream lost ({why}); reconnecting"))?;
                }
                let now = wait;
                wait = (wait * 2).min(LONGEST);
                now
            }
        };
        state.down = true;
        sleep(pause);
    }
}

/// One connection: until the stream ends or the Boxes change.
fn session(
    gate: &Gate,
    state: &mut State,
    say: &mut dyn FnMut(&str) -> io::Result<()>,
) -> io::Result<Ended> {
    match connect(gate, state) {
        Ok((token, stream)) => follow(gate, &token, stream, state, say),
        Err(lost) => Ok(Ended::Lost(lost)),
    }
}

/// Signs in, lists the Boxes and opens the listen.
fn connect(gate: &Gate, state: &mut State) -> Result<(String, Stream), Fail> {
    let token = gate.token()?;
    let listed = gate.call(&token, "resources/list", json!({}))?;
    let uris: Vec<String> = listed
        .get("resources")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|r| r.get("uri")?.as_str())
        .filter(|u| u.starts_with(PREFIX))
        .map(str::to_owned)
        .collect();
    if state.first {
        // What happened before this run started is history, not news.
        for uri in &uris {
            if let Some((key, _)) = news(gate, &token, uri) {
                state.said.insert(key);
            }
        }
        state.first = false;
    }
    let stream = gate.listen(&token, &uris)?;
    Ok((token, stream))
}

/// Says what the stream announces, until it ends.
fn follow(
    gate: &Gate,
    token: &str,
    stream: Stream,
    state: &mut State,
    say: &mut dyn FnMut(&str) -> io::Result<()>,
) -> io::Result<Ended> {
    for message in stream {
        let message = match message {
            Ok(m) => m,
            Err(e) => return Ok(Ended::Lost(Fail::Other(e.to_string()))),
        };
        match message.get("method").and_then(Value::as_str) {
            Some("notifications/subscriptions/acknowledged") if state.down => {
                state.down = false;
                say("moor-listen: reconnected")?;
            }
            Some("notifications/resources/list_changed") => return Ok(Ended::Relist),
            Some("notifications/resources/updated") => {
                let Some(uri) = message.pointer("/params/uri").and_then(Value::as_str) else {
                    continue;
                };
                if let Some((key, line)) = news(gate, token, uri) {
                    if state.said.insert(key) {
                        say(&line)?;
                    }
                }
            }
            _ => {}
        }
    }
    Ok(Ended::Lost(Fail::Other("stream ended".to_owned())))
}

/// The line for the newest event of the Box at `uri` that is worth one, with a key that is the
/// same for the same event. A Box that cannot be read says nothing: it may be gone.
fn news(gate: &Gate, token: &str, uri: &str) -> Option<(String, String)> {
    let read = gate
        .call(token, "resources/read", json!({"uri": uri}))
        .ok()?;
    let status: Value = serde_json::from_str(read.pointer("/contents/0/text")?.as_str()?).ok()?;
    let event = status.get("events")?.as_array()?.iter().rev().find(|e| {
        e.get("kind")
            .and_then(Value::as_str)
            .is_some_and(|k| WORTH.contains(&k))
    })?;
    let kind = event.get("kind")?.as_str()?;
    let key = format!("{uri}#{}", event.get("id")?);
    let name = uri.strip_prefix(PREFIX).unwrap_or(uri);
    let said = status
        .pointer("/agent_said/text")
        .and_then(Value::as_str)
        .and_then(|t| t.lines().next())
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(|t| t.chars().take(SAID).collect::<String>());
    let line = said.map_or_else(
        || format!("moor: box {name} {kind}"),
        |said| format!("moor: box {name} {kind}: {said}"),
    );
    Some((key, line))
}
