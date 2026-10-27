//! The Box's pane: the agent's terminal relayed in raw mode, then its card, until the user
//! closes the pane.

use std::io::{self, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use moor_client::{AttachKeys, ByteStream, Client, ClientError};
use moor_core::request::AttachReq;
use moor_core::response::Response;
use moor_core::view::{AttachEnd, AttachView};
use rustix::termios::{self, OptionalActions, Termios};
use serde_json::json;

use crate::boxes::{open_box, wake, Phase};
use crate::card::{Card, Choice, Ended, DETACH_BYTE};
use crate::error::Error;

/// What the pane's loop reacts to.
#[derive(Debug)]
pub enum Event {
    /// Bytes the user typed.
    Keys(Vec<u8>),
    /// The pane changed size.
    Resized(u16, u16),
    /// The relay's screen ended with this answer.
    Screen(Box<Result<Response, ClientError>>),
}

/// Relays one attach: `screen` goes to `out`, typed keys from `events` go to the agent until
/// [`DETACH_BYTE`], sizes follow resizes. Returns once the controller answers.
pub fn pump(
    mut screen: ByteStream,
    mut keys: AttachKeys,
    (tx, rx): (&Sender<Event>, &Receiver<Event>),
    mut out: impl Write + Send + 'static,
) -> Ended {
    let tx = tx.clone();
    thread::spawn(move || {
        let mut buf = vec![0; 16 * 1024];
        let copied = loop {
            match screen.read(&mut buf) {
                Ok(0) => break Ok(()),
                Ok(n) => {
                    let chunk = buf.get(..n).unwrap_or_default();
                    if let Err(e) = out.write_all(chunk).and_then(|()| out.flush()) {
                        break Err(ClientError::from(e));
                    }
                }
                Err(e) => break Err(ClientError::from(e)),
            }
        };
        let _ = tx.send(Event::Screen(Box::new(
            copied.and_then(|()| screen.finish()),
        )));
    });
    let mut detached = false;
    for event in rx {
        match event {
            Event::Keys(_) | Event::Resized(..) if detached => {}
            Event::Keys(bytes) => {
                let cut = bytes.iter().position(|b| *b == DETACH_BYTE);
                let _ = keys.send(bytes.get(..cut.unwrap_or(bytes.len())).unwrap_or_default());
                if cut.is_some() {
                    detached = true;
                    let _ = keys.detach();
                }
            }
            Event::Resized(cols, rows) => {
                let _ = keys.resize(cols, rows);
            }
            Event::Screen(answer) => return ended(*answer),
        }
    }
    Ended::Relay(AttachEnd::Link)
}

/// How a relay ended, from the controller's answer.
fn ended(answer: Result<Response, ClientError>) -> Ended {
    match answer {
        Ok(Response { error: Some(e), .. }) => Ended::Refused(e.message),
        Ok(Response { value, .. }) => value
            .and_then(|v| serde_json::from_value::<AttachView>(v).ok())
            .map_or(Ended::Relay(AttachEnd::Link), |v| Ended::Relay(v.ended_by)),
        Err(e) => Ended::Refused(e.to_string()),
    }
}

/// Raw mode on the pane's terminal, restored on drop.
struct Raw {
    /// The settings before raw mode.
    saved: Termios,
}

impl Raw {
    /// Puts stdin's terminal in raw mode; `None` when stdin is not a terminal.
    fn enter() -> io::Result<Option<Self>> {
        if !termios::isatty(io::stdin()) {
            return Ok(None);
        }
        let saved = termios::tcgetattr(io::stdin())?;
        let mut raw = saved.clone();
        raw.make_raw();
        termios::tcsetattr(io::stdin(), OptionalActions::Now, &raw)?;
        Ok(Some(Self { saved }))
    }
}

impl Drop for Raw {
    fn drop(&mut self) {
        let _ = termios::tcsetattr(io::stdin(), OptionalActions::Now, &self.saved);
    }
}

/// The pane's size, or 80 by 24 when it cannot be read.
fn size() -> (u16, u16) {
    termios::tcgetwinsize(io::stdout())
        .ok()
        .filter(|w| w.ws_col > 0 && w.ws_row > 0)
        .map_or((80, 24), |w| (w.ws_col, w.ws_row))
}

/// Feeds typed bytes and resizes into `tx` for the life of the pane.
fn feed(tx: &Sender<Event>) -> io::Result<()> {
    let keys = tx.clone();
    thread::spawn(move || {
        let mut buf = [0u8; 4096];
        while let Ok(n @ 1..) = io::stdin().read(&mut buf) {
            if keys
                .send(Event::Keys(buf.get(..n).unwrap_or_default().to_vec()))
                .is_err()
            {
                break;
            }
        }
    });
    let winch = Arc::new(AtomicBool::new(false));
    signal_hook::flag::register(signal_hook::consts::SIGWINCH, Arc::clone(&winch))?;
    let sizes = tx.clone();
    thread::spawn(move || loop {
        thread::sleep(Duration::from_millis(100));
        if winch.swap(false, Ordering::SeqCst) {
            let (cols, rows) = size();
            if sizes.send(Event::Resized(cols, rows)).is_err() {
                break;
            }
        }
    });
    Ok(())
}

/// Draws `card` over the whole pane, with a `note` under it when there is one.
fn draw(label: &str, card: &Card, note: Option<&str>) -> io::Result<()> {
    let mut out = io::stdout().lock();
    write!(
        out,
        "\x1b[0m\x1b[2J\x1b[H{}\r\n",
        card.lines(label).join("\r\n")
    )?;
    if let Some(note) = note {
        write!(out, "\r\n{note}\r\n")?;
    }
    out.flush()
}

/// Waits on the card for a key that does something; a note is drawn and waiting goes on.
fn choose(rx: &Receiver<Event>, label: &str, card: &Card) -> io::Result<Choice> {
    for event in rx {
        if let Event::Keys(bytes) = event {
            for b in bytes {
                match card.key(b) {
                    Choice::Stay => {}
                    Choice::Note(text) => draw(label, card, Some(text))?,
                    chosen => return Ok(chosen),
                }
            }
        }
    }
    Ok(Choice::Close)
}

/// How often a starting Box is asked whether it is up.
const POLL: Duration = Duration::from_secs(2);

/// Waits while `starting` says the Box is still being set up; `true` when the user closed
/// the pane meanwhile.
fn closed_while(rx: &Receiver<Event>, every: Duration, mut starting: impl FnMut() -> bool) -> bool {
    loop {
        match rx.recv_timeout(every) {
            Ok(Event::Keys(bytes)) => {
                if bytes
                    .iter()
                    .any(|b| Card::Starting.key(*b) == Choice::Close)
                {
                    return true;
                }
            }
            Ok(_) => {}
            Err(RecvTimeoutError::Timeout) => {
                if !starting() {
                    return false;
                }
            }
            Err(RecvTimeoutError::Disconnected) => return true,
        }
    }
}

/// Attaches once to Box `name`.
fn attach_once(client: &Client, name: &str, events: (&Sender<Event>, &Receiver<Event>)) -> Ended {
    let (cols, rows) = size();
    let req = serde_json::from_value::<AttachReq>(json!({"box": name, "cols": cols, "rows": rows}));
    let attached = req
        .map_err(Error::from)
        .and_then(|req| client.attach(req).map_err(Error::from));
    match attached {
        Ok((screen, keys)) => {
            let _ = io::stdout().write_all(b"\x1b[2J\x1b[H");
            pump(screen, keys, events, io::stdout())
        }
        Err(e) => Ended::Refused(e.to_string()),
    }
}

/// Runs the pane for Box `name` until the user closes it from the card.
pub fn run(client: &Client, name: &str) -> Result<(), Error> {
    let _raw = Raw::enter()?;
    let (tx, rx) = mpsc::channel();
    feed(&tx)?;
    let mut branch = None;
    let mut label = crate::boxes::label(None, name);
    loop {
        let card = match open_box(client, name) {
            Ok(before) => {
                if let Some(b) = &before {
                    branch = b.branch.clone().or(branch);
                    label = b.label();
                }
                if before.as_ref().is_some_and(|b| b.phase == Phase::Starting) {
                    draw(&label, &Card::Starting, None)?;
                    let starting = || {
                        open_box(client, name)
                            .is_ok_and(|b| b.is_some_and(|b| b.phase == Phase::Starting))
                    };
                    if closed_while(&rx, POLL, starting) {
                        return Ok(());
                    }
                    continue;
                }
                let ended = match before.map(|b| b.phase) {
                    Some(Phase::Live(_) | Phase::WaitingForMachine) => {
                        attach_once(client, name, (&tx, &rx))
                    }
                    _ => Ended::Skipped,
                };
                match open_box(client, name) {
                    Ok(now) => Card::after(&ended, now.as_ref(), branch.clone()),
                    Err(e) => Card::Refused(e.to_string()),
                }
            }
            Err(e) => Card::Refused(e.to_string()),
        };
        draw(&label, &card, None)?;
        match choose(&rx, &label, &card)? {
            Choice::Wake => {
                if let Err(e) = wake(client, name) {
                    let refused = Card::Refused(format!("wake: {e}"));
                    draw(&label, &refused, None)?;
                    if choose(&rx, &label, &refused)? == Choice::Close {
                        return Ok(());
                    }
                }
            }
            Choice::Close => return Ok(()),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests;
