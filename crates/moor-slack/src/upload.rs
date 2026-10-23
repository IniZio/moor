//! One artifact version in a Slack thread: the file itself, or one text line when it cannot go up.

use std::io::{self, Read};

use moor_client::{ByteStream, Client, ClientError};
use moor_core::request::{ArtifactReadReq, Request};
use moor_core::secret::Secret;
use moor_core::view::ArtifactView;
use moor_core::BoxName;
use sha2::{Digest, Sha256};

use crate::api::{Api, ApiError, UploadUrl};
use crate::escape::escape;

/// Slack's error code when the bot token lacks the `files:write` scope.
const MISSING_SCOPE: &str = "missing_scope";

/// The thread a version goes to, with the Slack client and bot token that post there.
pub(crate) struct Thread<'a> {
    /// Slack Web API client.
    pub(crate) api: &'a Api,
    /// Bot token.
    pub(crate) bot: &'a Secret,
    /// Channel id.
    pub(crate) channel: &'a str,
    /// Thread root `ts`.
    pub(crate) ts: &'a str,
}

/// Why a version did not go up.
enum NotUploaded {
    /// The bot token lacks `files:write`.
    MissingScope,
    /// Anything else, as a plain reason.
    Failed(String),
}

/// What a thread shows of one version after [`post_version`].
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Sent {
    /// The file, or its name and size when the bot token lacks `files:write`.
    Shown,
    /// Why the upload failed; the version is still owed.
    Failed(String),
}

/// Shows version `v` of Box `box_name` in `thread`: uploads the file, or posts one text line in
/// its place. A failed upload posts its reason unless `told` says the thread already shows
/// one. Errors only when a line it had to post did not reach the thread.
pub(crate) fn post_version(
    thread: &Thread<'_>,
    client: &Client,
    box_name: &BoxName,
    v: &ArtifactView,
    told: bool,
) -> Result<Sent, ApiError> {
    let name = escape(&v.name);
    let (text, sent) = match upload(thread, client, box_name, v) {
        Ok(()) => return Ok(Sent::Shown),
        Err(NotUploaded::MissingScope) => (format!("{name} ({})", size_text(v.size)), Sent::Shown),
        Err(NotUploaded::Failed(why)) if told => return Ok(Sent::Failed(why)),
        Err(NotUploaded::Failed(why)) => (
            format!("{name} could not be uploaded: {}", escape(&why)),
            Sent::Failed(why),
        ),
    };
    thread
        .api
        .post_message(thread.bot, thread.channel, thread.ts, &text)
        .map(|_| sent)
}

/// Streams the stored bytes of `v` to Slack, and shares the file in the thread only when they
/// match the listed size and sha256.
fn upload(
    thread: &Thread<'_>,
    client: &Client,
    box_name: &BoxName,
    v: &ArtifactView,
) -> Result<(), NotUploaded> {
    let to = thread
        .api
        .upload_url(thread.bot, &v.name, v.size)
        .map_err(refused)?;
    let read = Request::ArtifactRead(ArtifactReadReq {
        target: box_name.clone().into(),
        name: v.name.clone(),
        version: Some(v.version),
        offset: 0,
        len: v.size,
    });
    let stream = client.read_stream(read).map_err(|e| unreadable(&e))?;
    let mut body = Checked {
        stream,
        left: v.size,
        hash: Sha256::new(),
        failed: None,
    };
    let sent = thread.api.send_file(&to, v.size, &mut body);
    if let Some(why) = body.failed {
        return Err(NotUploaded::Failed(why));
    }
    sent.map_err(refused)?;
    match body.stream.read(&mut [0; 1]) {
        Ok(0) => {}
        Ok(_) => {
            return Err(NotUploaded::Failed(
                "the stored copy is longer than its listed size".to_owned(),
            ))
        }
        Err(e) => {
            return Err(NotUploaded::Failed(format!(
                "cannot read the stored copy: {e}"
            )))
        }
    }
    let reply = body.stream.finish().map_err(|e| unreadable(&e))?;
    if let Some(e) = reply.error {
        return Err(NotUploaded::Failed(format!(
            "cannot read the stored copy: {}",
            e.message
        )));
    }
    if !format!("{:x}", body.hash.finalize()).eq_ignore_ascii_case(&v.sha256) {
        return Err(NotUploaded::Failed(
            "the stored copy does not match its checksum".to_owned(),
        ));
    }
    complete(thread, &to, &v.name)
}

/// Shares the sent file in the thread.
fn complete(thread: &Thread<'_>, to: &UploadUrl, title: &str) -> Result<(), NotUploaded> {
    thread
        .api
        .complete_upload(thread.bot, thread.channel, thread.ts, to, title)
        .map_err(refused)
}

/// The reason for a Slack failure.
fn refused(e: ApiError) -> NotUploaded {
    match e {
        ApiError::Slack(code) if code == MISSING_SCOPE => NotUploaded::MissingScope,
        e => NotUploaded::Failed(e.to_string()),
    }
}

/// The reason for a controller read failure; it names no host path.
fn unreadable(e: &ClientError) -> NotUploaded {
    let why = match e {
        ClientError::Connect { .. } | ClientError::GaveUp { .. } => {
            "cannot reach the controller".to_owned()
        }
        ClientError::Frame { message, .. } => message.clone(),
        e => e.to_string(),
    };
    NotUploaded::Failed(format!("cannot read the stored copy: {why}"))
}

/// The upload body: the controller's bytes, cut at the listed size and hashed as they pass.
struct Checked {
    /// The controller's answer.
    stream: ByteStream,
    /// Bytes still owed by the listed size.
    left: u64,
    /// SHA-256 of the bytes read so far.
    hash: Sha256,
    /// Why reading the controller failed, once it has.
    failed: Option<String>,
}

impl Read for Checked {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() || self.left == 0 {
            return Ok(0);
        }
        let want = usize::try_from(self.left).map_or(buf.len(), |left| buf.len().min(left));
        let part = buf.get_mut(..want).unwrap_or_default();
        let err = match self.stream.read(part) {
            Ok(0) => io::Error::from(io::ErrorKind::UnexpectedEof),
            Ok(n) => {
                self.hash.update(part.get(..n).unwrap_or_default());
                self.left -= n as u64;
                return Ok(n);
            }
            Err(e) => e,
        };
        let why = match err.kind() {
            io::ErrorKind::UnexpectedEof => {
                "the stored copy is shorter than its listed size".to_owned()
            }
            _ => format!("cannot read the stored copy: {err}"),
        };
        self.failed = Some(why);
        Err(err)
    }
}

/// A size as moor-web shows it: bytes below 1 KB, else KB, MB or GB, with one decimal below 10.
fn size_text(n: u64) -> String {
    if n < 1024 {
        return format!("{n} B");
    }
    let n = u128::from(n);
    let (mut unit, mut base) = ("KB", 1024_u128);
    for next in ["MB", "GB"] {
        if n < base * 1024 {
            break;
        }
        (unit, base) = (next, base * 1024);
    }
    if n < base * 10 {
        let tenths = (n * 10 + base / 2) / base;
        format!("{}.{} {unit}", tenths / 10, tenths % 10)
    } else {
        format!("{} {unit}", (n + base / 2) / base)
    }
}

#[cfg(test)]
pub(crate) mod tests;
