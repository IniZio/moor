//! Requests that copy files between a Box and the host.

use serde::{Deserialize, Serialize};

use crate::name::{BoxId, BoxName};

/// Arguments of `cp`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CpReq {
    /// Box name.
    #[serde(rename = "box")]
    pub box_name: BoxName,
    /// Source path.
    pub src: String,
    /// Destination path on the host, absolute.
    pub dest: String,
}

/// Arguments of `box-mirror`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoxMirrorReq {
    /// The Box's store id.
    pub id: BoxId,
    /// Destination directory on the host, absolute.
    pub dest: String,
    /// rsync patterns left out of the mirror besides `.git`.
    #[serde(default)]
    pub exclude: Vec<String>,
}

/// Arguments of `box-put`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoxPutReq {
    /// The Box's store id.
    pub id: BoxId,
    /// Regular file on the host, absolute.
    pub src: String,
    /// Where the file lands, relative to the Box's work tree.
    pub dest: String,
}
