//! Artifact reads: the list of files a Box published and one file, streamed from the
//! controller and checked against its sha256 on the way through.

use std::io;

use moor_core::artifact::valid_name;
use moor_core::request::{ArtifactReadReq, ArtifactsReq, Request};
use moor_core::BoxName;
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::{call, envelope_value, field, items, json_ok, text};
use crate::client::Client;
use crate::http::{Download, Fill, HttpRequest, HttpResponse, Reply};

/// Content type used when the record's value is not a safe header value.
const FALLBACK_TYPE: &str = "application/octet-stream";

/// A `502` that carries no file contents.
fn bad_gateway(message: impl Into<String>) -> HttpResponse {
    HttpResponse::text("502 Bad Gateway", message)
}

/// Parses a Box name or returns the `400`.
fn parse_box(name: &str) -> Result<BoxName, HttpResponse> {
    BoxName::parse_allow_bare(name)
        .map_err(|_| HttpResponse::text("400 Bad Request", "bad box name"))
}

/// `GET /boxes/{name}/artifacts` with optional `?all=1`.
pub(super) fn list(client: &Client, req: &HttpRequest, name: &str) -> HttpResponse {
    let box_name = match parse_box(name) {
        Ok(box_name) => box_name,
        Err(resp) => return resp,
    };
    let all = req.query_param("all") == Some("1");
    match call(client, Request::Artifacts(ArtifactsReq { box_name, all })) {
        Ok(value) => json_ok(&value),
        Err(resp) => resp,
    }
}

/// The records in an `artifacts` result, whether it is a bare array or wraps one.
fn records(value: &Value) -> &[Value] {
    value
        .as_array()
        .map_or_else(|| items(value, "artifacts"), Vec::as_slice)
}

/// One listed version of an artifact.
struct Record {
    /// MIME type the Box published it with.
    content_type: String,
    /// Version number.
    version: u32,
    /// Size in bytes.
    size: u64,
    /// SHA-256 of the content, hex.
    sha256: String,
}

/// The record for `file`: version `v`, or the newest one.
fn find_record(value: &Value, file: &str, v: Option<u32>) -> Option<Record> {
    records(value)
        .iter()
        .filter(|r| text(r, "name") == Some(file))
        .filter_map(|r| {
            Some(Record {
                content_type: text(r, "content_type")?.to_owned(),
                version: u32::try_from(field(r, "version").as_u64()?).ok()?,
                size: field(r, "size").as_u64()?,
                sha256: text(r, "sha256")?.to_owned(),
            })
        })
        .filter(|r| v.is_none_or(|v| v == r.version))
        .max_by_key(|r| r.version)
}

/// Passes bytes straight to the body but holds back the file's last byte until the whole
/// file has checked out against its record, so a bad file never arrives complete.
struct Relay {
    /// Bytes the record says the file holds.
    want: u64,
    /// Bytes received so far.
    got: u64,
    /// Running digest of the bytes received.
    digest: Sha256,
    /// The file's last byte, once it has arrived.
    last: Option<u8>,
}

impl Relay {
    /// A relay for a file of `want` bytes.
    fn new(want: u64) -> Self {
        Self {
            want,
            got: 0,
            digest: Sha256::new(),
            last: None,
        }
    }

    /// Takes one frame and passes it to `out`, short of the file's last byte.
    fn push(
        &mut self,
        data: &[u8],
        out: &mut dyn FnMut(&[u8]) -> io::Result<()>,
    ) -> io::Result<()> {
        self.got += data.len() as u64;
        if self.got > self.want {
            return Err(io::Error::other("more bytes than the artifact holds"));
        }
        self.digest.update(data);
        match data.split_last() {
            Some((&last, rest)) if self.got == self.want => {
                self.last = Some(last);
                out(rest)
            }
            _ => out(data),
        }
    }

    /// Passes on the last byte if size and digest match `sha256`.
    fn finish(
        self,
        sha256: &str,
        out: &mut dyn FnMut(&[u8]) -> io::Result<()>,
    ) -> Result<(), HttpResponse> {
        if self.got != self.want {
            return Err(bad_gateway("artifact size mismatch"));
        }
        if !format!("{:x}", self.digest.finalize()).eq_ignore_ascii_case(sha256) {
            return Err(bad_gateway("artifact checksum mismatch"));
        }
        out(self.last.as_slice()).map_err(|e| bad_gateway(e.to_string()))
    }
}

/// Streams `read` from the controller into the body, checked against `record`.
fn fill(client: Client, read: ArtifactReadReq, record: Record) -> Fill {
    Box::new(move |out| {
        let mut relay = Relay::new(record.size);
        let envelope = client
            .read(Request::ArtifactRead(read), |data| relay.push(data, out))
            .map_err(|e| bad_gateway(e.to_string()))?;
        envelope_value(envelope)?;
        relay.finish(&record.sha256, out)
    })
}

/// Headers that keep the file from running as part of the page.
fn file_headers(content_type: &str, file: &str) -> Vec<(&'static str, String)> {
    let mut headers = vec![
        ("X-Content-Type-Options", "nosniff".to_owned()),
        ("Content-Security-Policy", "sandbox".to_owned()),
        ("Cache-Control", "no-store".to_owned()),
    ];
    if !(content_type.starts_with("image/") || content_type.starts_with("video/")) {
        headers.push((
            "Content-Disposition",
            format!("attachment; filename=\"{file}\""),
        ));
    }
    headers
}

/// `GET /boxes/{name}/artifacts/{file}` with optional `?v=N`.
pub(super) fn read(client: &Client, req: &HttpRequest, name: &str, file: &str) -> Reply {
    match download(client, req, name, file) {
        Ok(d) => Reply::Download(d),
        Err(resp) => resp.into(),
    }
}

/// The download for [`read`], or the error to answer with before any byte is read.
fn download(
    client: &Client,
    req: &HttpRequest,
    name: &str,
    file: &str,
) -> Result<Download, HttpResponse> {
    let box_name = parse_box(name)?;
    if !valid_name(file) {
        return Err(HttpResponse::text("400 Bad Request", "bad artifact name"));
    }
    let v = match req.query_param("v").map(str::parse::<u32>) {
        None => None,
        Some(Ok(v)) => Some(v),
        Some(Err(_)) => return Err(HttpResponse::text("400 Bad Request", "bad version")),
    };
    let listed = call(
        client,
        Request::Artifacts(ArtifactsReq {
            box_name: box_name.clone(),
            all: true,
        }),
    )?;
    let Some(record) = find_record(&listed, file, v) else {
        return Err(HttpResponse::text("404 Not Found", "no such artifact"));
    };
    let content_type = if record.content_type.chars().any(char::is_control) {
        FALLBACK_TYPE.to_owned()
    } else {
        record.content_type.clone()
    };
    let read = ArtifactReadReq {
        box_name,
        name: file.to_owned(),
        version: Some(record.version),
        offset: 0,
        len: record.size,
    };
    Ok(Download {
        status: "200 OK",
        headers: file_headers(&content_type, file),
        content_type,
        len: record.size,
        fill: fill(client.clone(), read, record),
    })
}

#[cfg(test)]
mod tests;
