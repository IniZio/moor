//! One Artifact a Box published, read from the controller as the caller: its record from
//! `artifacts`, then its bytes from `artifact-read`, whose answer after the last byte says
//! whether they matched that record's sha256.

use std::io;
use std::path::Path;

use moor_client::Client;
use moor_core::artifact::valid_name;
use moor_core::name::BoxName;
use moor_core::request::{ArtifactReadReq, ArtifactsReq, EnrollReq, Request};
use moor_core::response::{ErrorDetail, ErrorKind, ExitClass, Response};
use moor_core::view::ArtifactView;

use crate::{rewrite, Person};

/// The most an HTTP download holds back so that a sha256 mismatch can still be answered with
/// a status.
pub(crate) const ARTIFACT_HOLD_BACK_MAX: u64 = 4 * 1024 * 1024;

/// The Artifact a caller asked for.
pub(crate) struct Wanted {
    /// The Box that published it.
    pub(crate) r#box: BoxName,
    /// Its name.
    pub(crate) name: String,
    /// Its version; the newest when none.
    pub(crate) version: Option<u32>,
}

impl Wanted {
    /// The Artifact `name` of Box `r#box`, or why those do not name one.
    pub(crate) fn parse(r#box: &str, name: &str, version: Option<u32>) -> Result<Self, Refusal> {
        let r#box =
            BoxName::parse_allow_bare(r#box).map_err(|e| Refusal::BadRequest(e.to_string()))?;
        if !valid_name(name) {
            return Err(Refusal::BadRequest(format!(
                "invalid artifact name `{name}`"
            )));
        }
        Ok(Self {
            r#box,
            name: name.to_owned(),
            version,
        })
    }

    /// The gate path that downloads this Artifact at `version`.
    pub(crate) fn path(&self, version: u32) -> String {
        format!(
            "/boxes/{}/artifacts/{}/{version}",
            self.r#box.as_str(),
            self.name
        )
    }
}

/// Why an Artifact was not served.
#[derive(Debug)]
pub(crate) enum Refusal {
    /// The request does not name an Artifact.
    BadRequest(String),
    /// The controller would not let the caller read it.
    Forbidden(String),
    /// No such Box, name or version.
    NotFound(String),
    /// The Artifact is recorded but its stored bytes are gone.
    Gone(String),
    /// The stored bytes do not match the recorded sha256.
    Mismatch(String),
    /// The controller could not be asked, or failed.
    Failed(String),
}

impl Refusal {
    /// The HTTP status line that answers this refusal.
    pub(crate) const fn status(&self) -> &'static str {
        match self {
            Self::BadRequest(_) => "400 Bad Request",
            Self::Forbidden(_) => "403 Forbidden",
            Self::NotFound(_) => "404 Not Found",
            Self::Gone(_) => "410 Gone",
            Self::Mismatch(_) => "409 Conflict",
            Self::Failed(_) => "502 Bad Gateway",
        }
    }

    /// Why, in words.
    pub(crate) fn message(&self) -> &str {
        match self {
            Self::BadRequest(why)
            | Self::Forbidden(why)
            | Self::NotFound(why)
            | Self::Gone(why)
            | Self::Mismatch(why)
            | Self::Failed(why) => why,
        }
    }
}

/// An Artifact the controller holds a record of; [`read`] fetches its bytes.
pub(crate) struct Found {
    /// The Box that published it.
    target: BoxName,
    /// Its record: name, version, content type, size and sha256.
    pub(crate) record: ArtifactView,
}

/// The record of the Artifact `wanted` names, read as `person`.
pub(crate) fn find(socket: &Path, person: &Person, wanted: &Wanted) -> Result<Found, Refusal> {
    let client = Client::new(socket);
    let enroll = Request::Enroll(EnrollReq {
        identity: None,
        email: None,
    });
    ask(&client, person, enroll)?;
    let list = Request::Artifacts(ArtifactsReq {
        target: wanted.r#box.clone().into(),
        all: true,
    });
    let listed = ask(&client, person, list)?.value.unwrap_or_default();
    let rows: Vec<ArtifactView> = serde_json::from_value(listed).map_err(|e| {
        Refusal::Failed(format!("the controller's artifact list is unreadable: {e}"))
    })?;
    let record = rows
        .into_iter()
        .filter(|r| r.name == wanted.name && wanted.version.is_none_or(|v| v == r.version))
        .max_by_key(|r| r.version)
        .ok_or_else(|| {
            let which = wanted
                .version
                .map_or_else(String::new, |v| format!(" v{v}"));
            Refusal::NotFound(format!(
                "no artifact {}{which} for {}",
                wanted.name,
                wanted.r#box.as_str()
            ))
        })?;
    Ok(Found {
        target: wanted.r#box.clone(),
        record,
    })
}

/// Streams the whole of `found` to `on_bytes`, read as `person`.
pub(crate) fn read(
    socket: &Path,
    person: &Person,
    found: &Found,
    on_bytes: impl FnMut(&[u8]) -> io::Result<()>,
) -> Result<(), Refusal> {
    let request = Request::ArtifactRead(ArtifactReadReq {
        target: found.target.clone().into(),
        name: found.record.name.clone(),
        version: Some(found.record.version),
        offset: 0,
        len: found.record.size,
    });
    let acting = rewrite::act(request, person).map_err(Refusal::Failed)?;
    let reply = Client::new(socket)
        .read(acting, on_bytes)
        .map_err(|e| Refusal::Failed(format!("the artifact did not arrive whole: {e}")))?;
    refused(&reply).map_or(Ok(()), Err)
}

/// Runs `request` as `person`; the controller's reply, or why it refused.
fn ask(client: &Client, person: &Person, request: Request) -> Result<Response, Refusal> {
    let acting = rewrite::act(request, person).map_err(Refusal::Failed)?;
    let reply = client
        .request(acting, |_| {})
        .map_err(|e| Refusal::Failed(format!("the controller is unreachable: {e}")))?;
    refused(&reply).map_or(Ok(reply), Err)
}

/// The refusal a failed reply carries, read from its detail and kind, never its text.
fn refused(reply: &Response) -> Option<Refusal> {
    if reply.class == ExitClass::Ok {
        return None;
    }
    let Some(error) = &reply.error else {
        return Some(Refusal::Failed("the controller refused".to_owned()));
    };
    let why = error.message.clone();
    Some(match (&error.detail, error.kind) {
        (Some(ErrorDetail::ArtifactMismatch), _) => Refusal::Mismatch(why),
        (Some(ErrorDetail::ArtifactMissing), _) => Refusal::Gone(why),
        (_, ErrorKind::NotFound) => Refusal::NotFound(why),
        (_, ErrorKind::Usage) => Refusal::BadRequest(why),
        (_, ErrorKind::Refused) => Refusal::Forbidden(why),
        (_, ErrorKind::Failed | ErrorKind::Internal) => Refusal::Failed(why),
    })
}
