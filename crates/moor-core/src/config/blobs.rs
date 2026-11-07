//! The controller config's `blobs:` entry: where artifacts, reports and transcripts live.

use serde::Deserialize;

/// Which store holds blobs. The default keeps them under the state directory.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum BlobsConfig {
    /// Files under the state directory.
    #[default]
    Fs,
    /// A bucket behind any S3-compatible endpoint.
    S3(S3Blobs),
}

/// One bucket behind an S3-compatible endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct S3Blobs {
    /// The endpoint URL, `http` or `https`.
    pub endpoint: String,
    /// The bucket; moor never creates it.
    pub bucket: String,
    /// The signing region. None means `us-east-1`.
    pub region: Option<String>,
    /// Put before every key, so controllers can share a bucket.
    pub prefix: Option<String>,
    /// A directory holding the files `access_key_id` and `secret_access_key`, such as a mounted
    /// Secret. None reads `MOOR_BLOBS_ACCESS_KEY_ID` and `MOOR_BLOBS_SECRET_ACCESS_KEY`.
    pub credentials_dir: Option<String>,
}

impl BlobsConfig {
    /// Why this config cannot name a store: the key at fault and the reason.
    pub(super) fn problem(&self) -> Option<(&'static str, &'static str)> {
        let Self::S3(s3) = self else {
            return None;
        };
        if !s3.endpoint.starts_with("http://") && !s3.endpoint.starts_with("https://") {
            return Some(("blobs.endpoint", "must start with http:// or https://"));
        }
        s3.bucket
            .trim()
            .is_empty()
            .then_some(("blobs.bucket", "must not be empty"))
    }
}

#[cfg(test)]
mod tests;
