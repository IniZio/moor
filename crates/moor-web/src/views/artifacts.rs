//! Artifact reads: the list of files a Box published and one file, streamed from the
//! controller and checked against its sha256 on the way through.

use std::io;

use moor_core::artifact::valid_name;
use moor_core::request::{ArtifactReadReq, ArtifactsReq, Request};
use moor_core::BoxName;
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::{call, envelope_value, field, items, json_ok, text};
use crate::http::{Download, Fill, HttpRequest, HttpResponse, Reply};
use moor_client::Client;
use range::Wanted;

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

/// Passes bytes straight to the body but holds back the last byte until the whole body has
/// checked out, so a bad file never arrives complete.
struct Relay {
    /// Bytes the body holds.
    want: u64,
    /// Bytes received so far.
    got: u64,
    /// Running digest of a whole file and the sha256 its record names; a range that covers
    /// less than the whole file has none.
    check: Option<(Sha256, String)>,
    /// The body's last byte, once it has arrived.
    last: Option<u8>,
}

impl Relay {
    /// A relay for a body of `want` bytes, checked against `sha256` when given.
    fn new(want: u64, sha256: Option<String>) -> Self {
        Self {
            want,
            got: 0,
            check: sha256.map(|sha| (Sha256::new(), sha)),
            last: None,
        }
    }

    /// Takes one frame and passes it to `out`, short of the body's last byte.
    fn push(
        &mut self,
        data: &[u8],
        out: &mut dyn FnMut(&[u8]) -> io::Result<()>,
    ) -> io::Result<()> {
        self.got += data.len() as u64;
        if self.got > self.want {
            return Err(io::Error::other("more bytes than the artifact holds"));
        }
        if let Some((digest, _)) = &mut self.check {
            digest.update(data);
        }
        match data.split_last() {
            Some((&last, rest)) if self.got == self.want => {
                self.last = Some(last);
                out(rest)
            }
            _ => out(data),
        }
    }

    /// Passes on the last byte if the size, and the digest when checked, match.
    fn finish(self, out: &mut dyn FnMut(&[u8]) -> io::Result<()>) -> Result<(), HttpResponse> {
        if self.got != self.want {
            return Err(bad_gateway("artifact size mismatch"));
        }
        if let Some((digest, sha256)) = self.check {
            if !format!("{:x}", digest.finalize()).eq_ignore_ascii_case(&sha256) {
                return Err(bad_gateway("artifact checksum mismatch"));
            }
        }
        out(self.last.as_slice()).map_err(|e| bad_gateway(e.to_string()))
    }
}

/// Streams `read` from the controller into the body, checked against `sha256` when given.
fn fill(client: Client, read: ArtifactReadReq, sha256: Option<String>) -> Fill {
    Box::new(move |out| {
        let mut relay = Relay::new(read.len, sha256);
        let envelope = client
            .read(Request::ArtifactRead(read), |data| relay.push(data, out))
            .map_err(|e| bad_gateway(e.to_string()))?;
        envelope_value(envelope)?;
        relay.finish(out)
    })
}

/// Headers that keep the file from running as part of the page.
fn file_headers(content_type: &str, file: &str) -> Vec<(&'static str, String)> {
    let mut headers = vec![
        ("X-Content-Type-Options", "nosniff".to_owned()),
        ("Content-Security-Policy", "sandbox".to_owned()),
        ("Cache-Control", "no-store".to_owned()),
        ("Accept-Ranges", "bytes".to_owned()),
    ];
    if !(content_type.starts_with("image/") || content_type.starts_with("video/")) {
        headers.push((
            "Content-Disposition",
            format!("attachment; filename=\"{file}\""),
        ));
    }
    headers
}

/// `GET /boxes/{name}/artifacts/{file}` with optional `?v=N` and an optional single
/// `Range: bytes=` header.
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
    let mut headers = file_headers(&content_type, file);
    let size = record.size;
    let (status, offset, len, sha256) = match range::wanted(req.header("range"), size) {
        Wanted::Whole => ("200 OK", 0, size, Some(record.sha256)),
        Wanted::Part { start, end } => {
            headers.push(("Content-Range", format!("bytes {start}-{end}/{size}")));
            let whole = start == 0 && end + 1 == size;
            let sha256 = whole.then_some(record.sha256);
            ("206 Partial Content", start, end - start + 1, sha256)
        }
        Wanted::Unsatisfiable => {
            let mut resp = HttpResponse::text("416 Range Not Satisfiable", "bad range");
            resp.headers
                .push(("Content-Range", format!("bytes */{size}")));
            return Err(resp);
        }
    };
    let read = ArtifactReadReq {
        box_name,
        name: file.to_owned(),
        version: Some(record.version),
        offset,
        len,
    };
    Ok(Download {
        status,
        headers,
        content_type,
        len,
        fill: fill(client.clone(), read, sha256),
    })
}

mod range;
#[cfg(test)]
mod tests;
