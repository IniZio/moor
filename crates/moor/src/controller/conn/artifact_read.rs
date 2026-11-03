//! The `artifact-read` request: streams an Artifact's bytes over the connection.

use std::io::Write;

use moor_core::controller::write_bytes;
use moor_core::request::Grant;
use moor_core::{ArtifactReadReq, Response};

use crate::env::Env;
use crate::Moor;

/// Stream an Artifact's bytes as bytes frames for request `id`, then answer.
pub(super) fn read_artifact(
    env: &Env,
    grant: Grant,
    id: &str,
    req: &ArtifactReadReq,
    out: &mut dyn Write,
) -> Response {
    match Moor::from_env(env) {
        Ok(moor) => moor
            .granted(grant)
            .read_artifact(req, &mut |data| write_bytes(out, id, data)),
        Err(e) => e.into_response("artifact-read", vec![]),
    }
}
