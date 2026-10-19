//! Artifact reads: the list of files a Box published and one file, checked against its sha256.

use std::fmt::Write as _;

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use moor_core::artifact::valid_name;
use moor_core::request::{ArtifactReadReq, ArtifactsReq, Request, ARTIFACT_CHUNK};
use moor_core::BoxName;
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::{call, field, items, json_ok, text};
use crate::client::Client;
use crate::http::{HttpRequest, HttpResponse};

/// Content type used when the record's value is not a safe header value.
const FALLBACK_TYPE: &str = "application/octet-stream";

/// A `502` that carries no file contents.
fn bad_gateway(message: &'static str) -> HttpResponse {
    HttpResponse::text("502 Bad Gateway", message)
}

/// Parses a Box name or returns the `400`.
fn parse_box(name: &str) -> Result<BoxName, HttpResponse> {
    BoxName::parse_allow_bare(name)
        .map_err(|_| HttpResponse::text("400 Bad Request", "bad box name"))
}

/// `GET /boxes/{name}/artifacts` with optional `?all=1`.
pub(super) fn list(client: &Client, req: &HttpRequest, name: &str) -> HttpResponse {
    let task = match parse_box(name) {
        Ok(task) => task,
        Err(resp) => return resp,
    };
    let all = req.query_param("all") == Some("1");
    match call(
        client,
        Request::Artifacts(ArtifactsReq {
            box_name: task,
            all,
        }),
    ) {
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

/// Content type and version of the record for `file`: version `v`, or the newest one.
fn find_record(value: &Value, file: &str, v: Option<u32>) -> Option<(String, u32)> {
    records(value)
        .iter()
        .filter(|r| text(r, "name") == Some(file))
        .filter_map(|r| {
            let version = u32::try_from(field(r, "version").as_u64()?).ok()?;
            Some((text(r, "content_type")?.to_owned(), version))
        })
        .filter(|(_, version)| v.is_none_or(|v| v == *version))
        .max_by_key(|(_, version)| *version)
}

/// Lowercase hex of the sha256 of `data`.
fn sha256_hex(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .fold(String::with_capacity(64), |mut acc, b| {
            let _ = write!(acc, "{b:02x}");
            acc
        })
}

/// A complete, verified file.
struct Fetched {
    /// File bytes.
    bytes: Vec<u8>,
}

/// Reads every chunk of `file` at `version` and checks size and sha256.
fn fetch(
    client: &Client,
    task: &BoxName,
    file: &str,
    version: u32,
) -> Result<Fetched, HttpResponse> {
    let mut bytes: Vec<u8> = Vec::new();
    loop {
        let offset = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        let chunk = call(
            client,
            Request::ArtifactRead(ArtifactReadReq {
                box_name: task.clone(),
                name: file.to_owned(),
                version: Some(version),
                offset,
                len: ARTIFACT_CHUNK,
            }),
        )?;
        let data = text(&chunk, "data_b64")
            .and_then(|b| STANDARD.decode(b).ok())
            .ok_or_else(|| bad_gateway("bad artifact chunk"))?;
        let size = field(&chunk, "size").as_u64();
        let sha = text(&chunk, "sha256");
        let eof = field(&chunk, "eof").as_bool();
        let (Some(size), Some(sha), Some(eof)) = (size, sha, eof) else {
            return Err(bad_gateway("bad artifact chunk"));
        };
        if data.is_empty() && !eof {
            return Err(bad_gateway("bad artifact chunk"));
        }
        bytes.extend_from_slice(&data);
        let total = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        if total > size {
            return Err(bad_gateway("bad artifact chunk"));
        }
        if eof {
            let good = total == size && sha256_hex(&bytes).eq_ignore_ascii_case(sha);
            return if good {
                Ok(Fetched { bytes })
            } else {
                Err(bad_gateway("artifact checksum mismatch"))
            };
        }
    }
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
pub(super) fn read(client: &Client, req: &HttpRequest, name: &str, file: &str) -> HttpResponse {
    let task = match parse_box(name) {
        Ok(task) => task,
        Err(resp) => return resp,
    };
    if !valid_name(file) {
        return HttpResponse::text("400 Bad Request", "bad artifact name");
    }
    let v = match req.query_param("v").map(str::parse::<u32>) {
        None => None,
        Some(Ok(v)) => Some(v),
        Some(Err(_)) => return HttpResponse::text("400 Bad Request", "bad version"),
    };
    let listed = match call(
        client,
        Request::Artifacts(ArtifactsReq {
            box_name: task.clone(),
            all: true,
        }),
    ) {
        Ok(value) => value,
        Err(resp) => return resp,
    };
    let Some((content_type, version)) = find_record(&listed, file, v) else {
        return HttpResponse::text("404 Not Found", "no such artifact");
    };
    let content_type = if content_type.chars().any(char::is_control) {
        FALLBACK_TYPE.to_owned()
    } else {
        content_type
    };
    match fetch(client, &task, file, version) {
        Ok(f) => {
            let headers = file_headers(&content_type, file);
            HttpResponse::binary("200 OK", content_type, headers, f.bytes)
        }
        Err(resp) => resp,
    }
}

#[cfg(test)]
mod tests;
