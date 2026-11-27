//! The push guard: reads the ref updates at the head of a git push and refuses every ref the
//! Box does not own, ref deletion included.
//!
//! It knows git's receive-pack command list only, never a forge: a Box owns the one branch moor
//! ties it to, in each of its repos. Anything it cannot read is refused, never passed.

use std::io::{self, Read};

use gix_packetline::blocking_io::encode;
use gix_packetline::decode::{self, Stream};
use gix_packetline::{Channel, PacketLineRef};
use moor_core::domain::event::{EgressEvent, Event};

/// The contract key a refused push names.
pub(crate) const KEY: &str = "branch";

/// Most bytes of command list read before the push is refused unread.
const MAX_COMMANDS: usize = 1 << 20;

/// Most payload bytes in one side-band packet: 65520 less the length and the band byte.
const MAX_BAND_64K: usize = 65515;
/// Most payload bytes in one packet of the older side band.
const MAX_BAND: usize = 995;

/// One ref update a push asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RefUpdate {
    /// The object the ref names now, all zeros for a new ref.
    pub(crate) old: String,
    /// The object the ref is to name, all zeros to delete it.
    pub(crate) new: String,
    /// The full ref name, such as `refs/heads/main`.
    pub(crate) name: String,
}

impl RefUpdate {
    /// Whether the update deletes its ref.
    fn deletes(&self) -> bool {
        self.new.bytes().all(|b| b == b'0')
    }
}

/// The command list at the head of a push.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Commands {
    /// The ref updates, in the order the client sent them.
    pub(crate) updates: Vec<RefUpdate>,
    /// The capabilities the client asked for on its first update.
    pub(crate) caps: Vec<String>,
    /// The exact bytes read, through the flush, sent on first when the push passes.
    pub(crate) raw: Vec<u8>,
}

impl Commands {
    /// Whether the client asked for capability `name`.
    fn asks(&self, name: &str) -> bool {
        self.caps.iter().any(|c| c == name)
    }
}

/// The ref a Box owns: `refs/heads/<branch>` of the branch moor ties it to; none owns nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Owned(Option<String>);

impl Owned {
    /// The Box that pushes to `branch`.
    pub(crate) fn branch(branch: Option<&str>) -> Self {
        Self(
            branch
                .filter(|b| !b.is_empty())
                .map(|b| format!("refs/heads/{b}")),
        )
    }
}

/// One push the push guard refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PushRefusal {
    /// The git host.
    pub(crate) host: String,
    /// The repo path the push was for, such as `acme/widgets`.
    pub(crate) repo: String,
    /// The ref the refusal names, when the push was read that far.
    pub(crate) git_ref: Option<String>,
    /// What the client was told.
    pub(crate) reason: String,
}

impl PushRefusal {
    /// The Box event that names the refused push and the contract key it keeps.
    pub(crate) fn event(&self) -> Event {
        Event::Egress(EgressEvent::PushRefused {
            host: self.host.clone(),
            repo: self.repo.clone(),
            git_ref: self.git_ref.clone(),
            key: KEY.to_owned(),
            reason: self.reason.clone(),
        })
    }
}

/// Why the guard refuses a push.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Refused {
    /// The push deletes this ref.
    Deletes(String),
    /// The push updates this ref, which the Box does not own.
    NotOwned(String),
    /// The command list could not be read, for this reason.
    Unreadable(&'static str),
}

impl Refused {
    /// The ref the refusal names, when it got that far.
    pub(crate) fn git_ref(&self) -> Option<&str> {
        match self {
            Self::Deletes(r) | Self::NotOwned(r) => Some(r),
            Self::Unreadable(_) => None,
        }
    }

    /// What the person and the agent read: the ref, why, and the contract key.
    pub(crate) fn message(&self, owned: &Owned) -> String {
        let why = match (self, &owned.0) {
            (Self::Deletes(r), _) => format!("moor refuses to delete {r}; a Box deletes no ref"),
            (Self::NotOwned(r), Some(mine)) => {
                format!("moor refuses a push to {r}; this Box pushes only to {mine}")
            }
            (Self::NotOwned(r), None) => {
                format!("moor refuses a push to {r}; this Box owns no branch")
            }
            (Self::Unreadable(why), _) => format!("moor refuses this push; {why}"),
        };
        format!("{why} (contract: {KEY})")
    }
}

/// Reads the command list of a receive-pack request from `r`, exactly through its flush, so the
/// pack after it stays unread.
///
/// # Errors
/// [`Refused::Unreadable`] for a list that ends early, runs past [`MAX_COMMANDS`], is signed, or
/// holds a line that is not a ref update.
pub(crate) fn read_commands(r: &mut impl Read) -> Result<Commands, Refused> {
    let mut raw = Vec::new();
    let mut updates = Vec::new();
    let mut caps = Vec::new();
    loop {
        let Some(data) = next_line(r, &mut raw)? else {
            return Ok(Commands { updates, caps, raw });
        };
        let line = data.strip_suffix(b"\n").unwrap_or(&data);
        let (command, asked) = line
            .iter()
            .position(|&b| b == 0)
            .map_or((line, None), |nul| {
                (line.get(..nul).unwrap_or_default(), line.get(nul + 1..))
            });
        if command.starts_with(b"shallow ") {
            continue;
        }
        if command.starts_with(b"push-cert") {
            return Err(Refused::Unreadable("moor cannot read a signed push"));
        }
        if let Some(asked) = asked.filter(|_| updates.is_empty()) {
            caps = String::from_utf8_lossy(asked)
                .split(' ')
                .filter(|c| !c.is_empty())
                .map(str::to_owned)
                .collect();
        }
        updates.push(ref_update(command).ok_or(Refused::Unreadable(
            "the push holds a line that is not a ref update",
        ))?);
    }
}

/// Reads one pkt-line onto `raw`: its data, or `None` at the flush.
fn next_line(r: &mut impl Read, raw: &mut Vec<u8>) -> Result<Option<Vec<u8>>, Refused> {
    let start = raw.len();
    let mut need = 4;
    loop {
        if raw.len() + need > MAX_COMMANDS {
            return Err(Refused::Unreadable("the push's ref updates run too long"));
        }
        let at = raw.len();
        raw.resize(at + need, 0);
        let got = raw.get_mut(at..).map_or(Ok(()), |buf| r.read_exact(buf));
        if got.is_err() {
            return Err(Refused::Unreadable(
                "the push ended before its ref updates did",
            ));
        }
        let unreadable = Refused::Unreadable("the push's ref updates are not pkt-lines");
        match decode::streaming(raw.get(start..).unwrap_or_default()) {
            Ok(Stream::Complete { line, .. }) => {
                return match line {
                    PacketLineRef::Data(d) => Ok(Some(d.to_vec())),
                    PacketLineRef::Flush => Ok(None),
                    PacketLineRef::Delimiter | PacketLineRef::ResponseEnd => Err(unreadable),
                };
            }
            Ok(Stream::Incomplete { bytes_needed }) => need = bytes_needed,
            Err(_) => return Err(unreadable),
        }
    }
}

/// `old new name`, with object ids of 40 or 64 hex digits and a name under `refs/`.
fn ref_update(command: &[u8]) -> Option<RefUpdate> {
    let text = std::str::from_utf8(command).ok()?;
    let mut parts = text.split(' ');
    let (old, new, name) = (parts.next()?, parts.next()?, parts.next()?);
    let oid = |s: &str| matches!(s.len(), 40 | 64) && s.bytes().all(|b| b.is_ascii_hexdigit());
    let named = name.starts_with("refs/") && name.len() > 5;
    (parts.next().is_none() && oid(old) && oid(new) && named).then(|| RefUpdate {
        old: old.to_owned(),
        new: new.to_owned(),
        name: name.to_owned(),
    })
}

/// Passes a push whose every update moves the ref the Box owns; refuses the first that does not.
///
/// # Errors
/// [`Refused::Deletes`] or [`Refused::NotOwned`] for the first update the Box may not make.
pub(crate) fn judge(updates: &[RefUpdate], owned: &Owned) -> Result<(), Refused> {
    for u in updates {
        if u.deletes() {
            return Err(Refused::Deletes(u.name.clone()));
        }
        if owned.0.as_deref() != Some(u.name.as_str()) {
            return Err(Refused::NotOwned(u.name.clone()));
        }
    }
    Ok(())
}

/// The receive-pack result that refuses every update of `commands` with `message`, as git shows a
/// hook's refusal; `None` when the client asked for no status report to carry it.
pub(crate) fn report(commands: &Commands, message: &str) -> Option<Vec<u8>> {
    if !commands.asks("report-status") && !commands.asks("report-status-v2") {
        return None;
    }
    let mut status = Vec::new();
    write_report(commands, message, &mut status).ok()?;
    let band = if commands.asks("side-band-64k") {
        MAX_BAND_64K
    } else if commands.asks("side-band") {
        MAX_BAND
    } else {
        return Some(status);
    };
    let mut out = Vec::new();
    encode::band_to_write(
        Channel::Progress,
        format!("{message}\n").as_bytes(),
        &mut out,
    )
    .ok()?;
    for part in status.chunks(band) {
        encode::band_to_write(Channel::Data, part, &mut out).ok()?;
    }
    encode::flush_to_write(&mut out).ok()?;
    Some(out)
}

/// Writes `unpack ok`, one `ng` line per update and the flush.
fn write_report(commands: &Commands, message: &str, out: &mut Vec<u8>) -> io::Result<()> {
    encode::text_to_write(b"unpack ok", &mut *out)?;
    for u in &commands.updates {
        encode::text_to_write(format!("ng {} {message}", u.name).as_bytes(), &mut *out)?;
    }
    encode::flush_to_write(out).map(drop)
}

#[cfg(test)]
mod tests;
