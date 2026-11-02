//! The watch loop: list the Boxes, listen on them, read each one that changed and say what
//! happened, and start over when the stream drops.

use std::collections::HashSet;
use std::io;
use std::thread::sleep;
use std::time::Duration;

use serde_json::{json, Value};

use crate::gate::{Gate, Stream};
use crate::token::Fail;

/// Every Box resource's URI starts with this.
const PREFIX: &str = "moor://box/";

/// Events worth a line; the rest (busy, ready, asleep) are noise to a reader.
const WORTH: [&str; 10] = [
    "idled",
    "needs_input",
    "agent_stalled",
    "agent_resumed",
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

/// A stream that went and has not come back.
struct Outage {
    /// Why it went, in words for a log line.
    why: String,
    /// Whether a line told the reader about it.
    told: bool,
}

/// What carries over between connections.
struct State {
    /// Events already said, as `uri#id`.
    said: HashSet<String>,
    /// Whether the first connection has not yet listed the Boxes.
    first: bool,
    /// The outage under way, if any.
    outage: Option<Outage>,
}

/// Runs until `say` fails, which is when nobody reads the lines any more. `signed_out` is how
/// long to wait before looking again for a sign-in.
pub(crate) fn run(
    gate: &Gate,
    signed_out: Duration,
    say: &mut dyn FnMut(&str) -> io::Result<()>,
) -> io::Result<()> {
    let mut state = State {
        said: HashSet::new(),
        first: true,
        outage: None,
    };
    let mut wait = FIRST;
    loop {
        let lost = match session(gate, &mut state, say)? {
            Ended::Relist => continue,
            Ended::Lost(lost) => lost,
        };
        let pause = match lost {
            Fail::Rejected => {
                if !state.outage.as_ref().is_some_and(|o| o.told) {
                    say("moor-listen: token rejected; call any moor tool so the client refreshes it")?;
                }
                state.outage = Some(Outage {
                    why: "token rejected".to_owned(),
                    told: true,
                });
                LONGEST
            }
            Fail::SignedOut(server) => {
                if !state.outage.as_ref().is_some_and(|o| o.told) {
                    say(&format!(
                        "moor-listen: not signed in to {server}; run moor login --server {server}"
                    ))?;
                }
                state.outage = Some(Outage {
                    why: "not signed in".to_owned(),
                    told: true,
                });
                signed_out
            }
            Fail::Other(why) => {
                if state.outage.is_none() {
                    wait = FIRST;
                }
                went(&mut state, why, say)?;
                let now = wait;
                wait = (wait * 2).min(LONGEST);
                now
            }
        };
        sleep(pause);
    }
}

/// Notes a lost stream. The first loss stays quiet, since the reconnect usually works; a
/// failed reconnect tells the reader.
fn went(
    state: &mut State,
    why: String,
    say: &mut dyn FnMut(&str) -> io::Result<()>,
) -> io::Result<()> {
    match &mut state.outage {
        None => state.outage = Some(Outage { why, told: false }),
        Some(outage) if !outage.told => {
            say(&format!(
                "moor-listen: stream lost ({}); reconnecting",
                outage.why
            ))?;
            outage.told = true;
        }
        Some(_) => {}
    }
    Ok(())
}

/// One connection: until the stream ends or the Boxes change.
fn session(
    gate: &Gate,
    state: &mut State,
    say: &mut dyn FnMut(&str) -> io::Result<()>,
) -> io::Result<Ended> {
    match connect(gate, state) {
        Ok(open) => follow(gate, open, state, say),
        Err(lost) => Ok(Ended::Lost(lost)),
    }
}

/// A listen that is open, with what it was opened with.
struct Open {
    /// The token it signed in with.
    token: String,
    /// The Boxes it listens on.
    uris: Vec<String>,
    /// Its messages.
    stream: Stream,
}

/// Signs in, lists the Boxes and opens the listen.
fn connect(gate: &Gate, state: &mut State) -> Result<Open, Fail> {
    let mut token = gate.token()?;
    let listed = match gate.call(&token, "resources/list", json!({})) {
        // A token the gate refused may only be stale; ask for a new one once.
        Err(Fail::Rejected) => {
            token = gate.renew()?;
            gate.call(&token, "resources/list", json!({}))?
        }
        other => other?,
    };
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
    Ok(Open {
        token,
        uris,
        stream,
    })
}

/// Says what the stream announces, until it ends.
fn follow(
    gate: &Gate,
    open: Open,
    state: &mut State,
    say: &mut dyn FnMut(&str) -> io::Result<()>,
) -> io::Result<Ended> {
    let Open {
        token,
        uris,
        stream,
    } = open;
    for message in stream {
        let message = match message {
            Ok(m) => m,
            Err(e) => return Ok(Ended::Lost(Fail::Other(e.to_string()))),
        };
        match message.get("method").and_then(Value::as_str) {
            Some("notifications/subscriptions/acknowledged") => {
                if state.outage.take().is_some_and(|o| o.told) {
                    say("moor-listen: reconnected")?;
                }
                // Boxes may have changed while no stream was open.
                for uri in &uris {
                    tell(gate, &token, uri, state, say)?;
                }
            }
            Some("notifications/resources/list_changed") => return Ok(Ended::Relist),
            Some("notifications/resources/updated") => {
                if let Some(uri) = message.pointer("/params/uri").and_then(Value::as_str) {
                    tell(gate, &token, uri, state, say)?;
                }
            }
            _ => {}
        }
    }
    Ok(Ended::Lost(Fail::Other("stream ended".to_owned())))
}

/// Says the newest event of the Box at `uri` worth a line, unless it was said already.
fn tell(
    gate: &Gate,
    token: &str,
    uri: &str,
    state: &mut State,
    say: &mut dyn FnMut(&str) -> io::Result<()>,
) -> io::Result<()> {
    if let Some((key, line)) = news(gate, token, uri) {
        if state.said.insert(key) {
            say(&line)?;
        }
    }
    Ok(())
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
    let line = quiet_line(name, kind, event.get("payload")).unwrap_or_else(|| {
        said.map_or_else(
            || format!("moor: box {name} {kind}"),
            |said| format!("moor: box {name} {kind}: {said}"),
        )
    });
    Some((key, line))
}

/// The line for a stall or its end, from the event's payload; none for other kinds.
fn quiet_line(name: &str, kind: &str, payload: Option<&Value>) -> Option<String> {
    let mins = payload?.get("quiet_secs")?.as_u64()? / 60;
    match kind {
        "agent_stalled" => {
            let doing = payload?.get("doing").and_then(Value::as_str);
            Some(doing.filter(|d| !d.is_empty()).map_or_else(
                || format!("moor: box {name} stalled: no progress for {mins} min"),
                |d| format!("moor: box {name} stalled: no progress for {mins} min while {d}"),
            ))
        }
        "agent_resumed" => Some(format!("moor: box {name} resumed after {mins} min")),
        _ => None,
    }
}
