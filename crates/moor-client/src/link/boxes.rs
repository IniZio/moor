//! The Box protocol: what the controller asks of a machine that provides Boxes, and what the
//! machine answers.
//!
//! One stream carries one request. The runner opens it with the fixed line [`Op::Boxes`] and then
//! splices it without reading, so the controller's [`BoxOp`] line is the next line the agent reads;
//! its answers are [`BoxAnswer`] lines. After a `streaming` answer the stream carries
//! [`super::chan`] frames for an `exec` and raw bytes for a `forward`.
//!
//! A verb or field added later bumps [`PROTOCOL`], and a controller sends it only to a machine
//! whose [`BoxOffer`] is at or above that number. Requests refuse unknown fields, so a request too
//! new for an agent is refused, never misread. Answers and reports ignore unknown fields.
//!
//! [`Op::Boxes`]: super::ops::Op::Boxes

use moor_core::box_key::ControllerId;
use moor_core::seam::driver::Place;
use serde::{Deserialize, Serialize};

use super::lease::Cache;

/// The Box protocol's version; it grows when a verb or a field is added.
pub const PROTOCOL: u16 = 1;

/// What a machine that provides Boxes says of itself in its offer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoxOffer {
    /// The [`PROTOCOL`] it speaks.
    pub protocol: u16,
    /// How it provides Boxes, such as `docker`; for people to read, never branched on.
    pub provides: String,
}

/// The controller's one request on a Box stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "verb", deny_unknown_fields)]
pub enum BoxOp {
    /// Make whatever of the Box is missing and start what is stopped; repeating it finds the
    /// same Box. A second open joins the one in flight.
    Open {
        /// `moor-<controller>-<id>[-h<inc>]`.
        slug: String,
        /// The machine class; none leaves it to the machine.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        class: Option<String>,
        /// The cache volumes the work place mounts.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cache: Option<Cache>,
    },
    /// Start a sleeping Box.
    Wake {
        /// The Box.
        slug: String,
    },
    /// Stop every process in the Box, keeping its files.
    Sleep {
        /// The Box.
        slug: String,
    },
    /// Remove the Box and what it owns; one already gone is fine.
    Close {
        /// The Box.
        slug: String,
    },
    /// Record the work place as it is now, to start a later repair from.
    Seal {
        /// The Box.
        slug: String,
    },
    /// Make `vars` and the CA the environment of every later command in both places; the new
    /// set replaces the old.
    Env {
        /// The Box.
        slug: String,
        /// Exported before every command.
        vars: Vec<(String, String)>,
        /// A CA certificate, PEM, both places trust.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ca_pem: Option<String>,
    },
    /// Report this controller's Boxes.
    List {
        /// The controller whose slugs name the Boxes.
        controller: ControllerId,
    },
    /// Run a command in one place of the Box; the stream then carries frames.
    Exec {
        /// The Box.
        slug: String,
        /// Where in it.
        place: Place,
        /// Program and arguments.
        argv: Vec<String>,
        /// Exported before the command.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        env: Vec<(String, String)>,
        /// The directory it runs in; the Box's workdir when none.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<String>,
        /// Whether `in` frames follow; without it the command's stdin is empty.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        stdin: bool,
        /// Run it on a terminal of this size; its output is all `out` frames.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tty: Option<Size>,
    },
    /// Reach a TCP port on the Box's loopback.
    Forward {
        /// The Box.
        slug: String,
        /// The port.
        port: u16,
        /// Only ask whether anything listens; no bytes follow.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        probe: bool,
    },
}

impl BoxOp {
    /// The verb's name on the wire.
    #[must_use]
    pub const fn verb(&self) -> &'static str {
        match self {
            Self::Open { .. } => "open",
            Self::Wake { .. } => "wake",
            Self::Sleep { .. } => "sleep",
            Self::Close { .. } => "close",
            Self::Seal { .. } => "seal",
            Self::Env { .. } => "env",
            Self::List { .. } => "list",
            Self::Exec { .. } => "exec",
            Self::Forward { .. } => "forward",
        }
    }
}

/// The machine's answer lines.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "answer")]
pub enum BoxAnswer {
    /// Progress while an open works, such as an image being pulled; any number come first.
    Note {
        /// What it is doing.
        line: String,
    },
    /// The Box is made and started.
    Opened {
        /// The workdir, the same path with the same files in both places.
        workdir: String,
        /// The Box's generation: it grows whenever the work place's processes were lost.
        generation: u32,
    },
    /// The Box is awake again.
    Woke {
        /// The Box's generation now.
        generation: u32,
    },
    /// The verb took effect.
    Done,
    /// The machine no longer has this Box.
    Gone,
    /// This controller's Boxes.
    Listed {
        /// Each Box.
        boxes: Vec<BoxReport>,
    },
    /// The command or forward runs; frames or bytes follow.
    Streaming,
    /// Whether anything listens on the probed port.
    Listens {
        /// Whether it does.
        yes: bool,
    },
    /// The request was not served.
    Refused {
        /// Why.
        reason: String,
    },
}

/// A terminal's size in character cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Size {
    /// Columns.
    pub cols: u16,
    /// Rows.
    pub rows: u16,
}

/// One Box's health as its machine reports it: level state, so a report missed costs only
/// latency.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoxReport {
    /// The Box.
    pub slug: String,
    /// What it is doing.
    pub state: ReportState,
    /// How many times the work place's processes were lost.
    pub generation: u32,
    /// How many processes the work place's memory limit killed.
    pub oom_kills: u32,
    /// The machine's latest repair of the Box.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_mend: Option<ReportMend>,
    /// MiB of the host the Box holds, when its machine can tell.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mem_mib: Option<u64>,
    /// Why a Box meant to run is down, when its machine saw its places stop on their own: each
    /// place that stopped, its exit code and whether its memory limit killed it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub down: Option<String>,
}

/// What a Box is doing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReportState {
    /// Running.
    Awake,
    /// Stopped on request.
    Asleep,
    /// Its machine is bringing it back.
    Mending,
    /// Its machine gave up on it.
    Broken {
        /// Why.
        why: String,
    },
}

/// One repair of a Box.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportMend {
    /// What was wrong, such as `out-of-memory`.
    pub why: String,
    /// What was done, such as `restart`.
    pub how: String,
    /// When, in unix seconds.
    pub at: i64,
}

#[cfg(test)]
mod tests;
