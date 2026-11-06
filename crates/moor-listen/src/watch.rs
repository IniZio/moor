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
const WORTH: [&str; 13] = [
    "idled",
    "needs_input",
    "agent_stalled",
    "agent_resumed",
    "node_disk_low",
    "node_disk_recovered",
    "setup_failed",
    "session_failed",
    "launch_timed_out",
    "box_failed",
    "relaunch_capped",
    "disk_lost",
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
    /// Every Box asked for has closed; nothing is left to watch.
    Done,
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
    /// Names of the only Boxes to watch; empty watches them all.
    only: Vec<String>,
    /// Watched Boxes whose `closed` has been said.
    closed: HashSet<String>,
    /// Watched Boxes already reported as not visible.
    missing: HashSet<String>,
}

impl State {
    /// Whether Boxes were asked for by name and each has closed.
    fn finished(&self) -> bool {
        !self.only.is_empty() && self.only.iter().all(|n| self.closed.contains(n))
    }
}

/// Runs until `say` fails, which is when nobody reads the lines any more, or until every Box in
/// `only` has closed. An empty `only` watches every Box. `signed_out` is how long to wait before
/// looking again for a sign-in.
pub(crate) fn run(
    gate: &Gate,
    signed_out: Duration,
    only: Vec<String>,
    say: &mut dyn FnMut(&str) -> io::Result<()>,
) -> io::Result<()> {
    let mut state = State {
        said: HashSet::new(),
        first: true,
        outage: None,
        only,
        closed: HashSet::new(),
        missing: HashSet::new(),
    };
    let mut wait = FIRST;
    loop {
        let lost = match session(gate, &mut state, say)? {
            Ended::Relist => continue,
            Ended::Done => return Ok(()),
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
        Ok(open) => {
            for name in &open.absent {
                if state.missing.insert(name.clone()) {
                    say(&format!(
                        "moor-listen: no Box named {name} is visible; waiting for it"
                    ))?;
                }
            }
            follow(gate, open, state, say)
        }
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
    /// Asked-for Boxes the server did not list and that have not closed.
    absent: Vec<String>,
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
        .filter(|u| {
            state.only.is_empty() || state.only.iter().any(|n| u.strip_prefix(PREFIX) == Some(n))
        })
        .map(str::to_owned)
        .collect();
    // A closed Box leaves the list, so only the others count as absent.
    let absent = state
        .only
        .iter()
        .filter(|n| {
            !state.closed.contains(*n) && !uris.iter().any(|u| u.strip_prefix(PREFIX) == Some(n))
        })
        .cloned()
        .collect();
    if state.first {
        // What happened before this run started is history, not news.
        for uri in &uris {
            if let Some((key, ..)) = news(gate, &token, uri) {
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
        absent,
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
        ..
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
        if state.finished() {
            return Ok(Ended::Done);
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
    if let Some((key, line, closed)) = news(gate, token, uri) {
        if state.said.insert(key) {
            say(&line)?;
            if closed {
                state
                    .closed
                    .insert(uri.strip_prefix(PREFIX).unwrap_or(uri).to_owned());
            }
        }
    }
    Ok(())
}

/// The line for the newest event of the Box at `uri` that is worth one, with a key that is the
/// same for the same event, and whether that event is the Box closing. A Box that cannot be read says nothing: it may be gone.
fn news(gate: &Gate, token: &str, uri: &str) -> Option<(String, String, bool)> {
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
    let payload = event.get("payload");
    let line = quiet_line(name, kind, payload)
        .or_else(|| disk_line(name, kind, payload))
        .unwrap_or_else(|| {
            said.map_or_else(
                || format!("moor: box {name} {kind}"),
                |said| format!("moor: box {name} {kind}: {said}"),
            )
        });
    Some((key, line, kind == "closed"))
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

/// The line for a node's disk running low or coming back; none for other kinds.
fn disk_line(name: &str, kind: &str, payload: Option<&Value>) -> Option<String> {
    if kind == "disk_lost" {
        return Some(format!(
            "moor: box {name} disk_lost: its machine was replaced, so anything not pushed is gone"
        ));
    }
    if !matches!(kind, "node_disk_low" | "node_disk_recovered") {
        return None;
    }
    let payload = payload?;
    let node = payload.get("node")?.as_str()?;
    let free = payload.get("free_mib")?.as_u64()? / 1024;
    let total = payload.get("total_mib")?.as_u64()? / 1024;
    Some(format!(
        "moor: box {name} {kind}: {node} has {free} GiB free of {total} GiB"
    ))
}
