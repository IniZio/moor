//! Blobs in an object store, driven from blocking callers through a private runtime.

use std::fs::{self, File};
use std::io::{self, Read};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt as _;
use moor_core::config::S3Blobs;
use object_store::aws::AmazonS3Builder;
use object_store::path::Path as ObjectPath;
use object_store::prefix::PrefixStore;
use object_store::{
    BackoffConfig, ClientOptions, GetOptions, GetRange, ObjectStore, ObjectStoreExt as _,
    RetryConfig, WriteMultipart,
};
use tokio::runtime::Runtime;

use super::{BlobError, BlobKey, Blobs};
use crate::env::{Env, BLOBS_ACCESS_KEY_ID, BLOBS_SECRET_ACCESS_KEY};

/// Bytes per uploaded part; a file no larger goes up in one request.
const PART: usize = 16 * 1024 * 1024;
/// Parts in flight at once, which also caps the memory one upload holds.
const PARTS_IN_FLIGHT: usize = 4;
/// Size of the buffer a file passes through on its way up.
const CHUNK: usize = 64 * 1024;
/// Signing region when the config names none; S3-compatible stores accept any.
const DEFAULT_REGION: &str = "us-east-1";

/// An access key pair.
pub(super) struct Credentials {
    /// The key id.
    pub(super) id: String,
    /// The secret.
    pub(super) secret: String,
}

/// The key pair `config` names: files in its credentials directory, else the environment.
pub(super) fn credentials(config: &S3Blobs, env: &Env) -> Result<Credentials, String> {
    let (id, secret) = if let Some(dir) = &config.credentials_dir {
        let dir = env.expand_home(Path::new(dir));
        let read = |name: &str| {
            let path = dir.join(name);
            fs::read_to_string(&path)
                .map(|text| text.trim().to_owned())
                .map_err(|e| format!("blobs: cannot read {}: {e}", path.display()))
        };
        (read("access_key_id")?, read("secret_access_key")?)
    } else {
        let read = |name: &str| {
            env.get(name)
                .map(str::to_owned)
                .ok_or_else(|| format!("blobs: set {name} or credentials_dir"))
        };
        (read(BLOBS_ACCESS_KEY_ID)?, read(BLOBS_SECRET_ACCESS_KEY)?)
    };
    Ok(Credentials { id, secret })
}

/// Blobs in one bucket.
pub(super) struct ObjectBlobs {
    /// The bucket, under the configured prefix.
    store: Arc<dyn ObjectStore>,
    /// Runs the client's requests; callers block on it, as moor has no async callers.
    runtime: Runtime,
    /// How errors name the store.
    at: String,
}

impl ObjectBlobs {
    /// The bucket `config` names. It makes no request, so an unreachable store shows at first use.
    pub(super) fn s3(config: &S3Blobs, credentials: &Credentials) -> Result<Self, String> {
        // A whole-request limit would cut a large artifact, so a stalled read has the limit.
        let client = ClientOptions::new()
            .with_allow_http(config.endpoint.starts_with("http://"))
            .with_connect_timeout(Duration::from_secs(5))
            .with_timeout_disabled()
            .with_read_timeout(Duration::from_secs(60));
        // The library's default keeps at it for 3 minutes, which a waiting reader would feel.
        let retry = RetryConfig {
            backoff: BackoffConfig::default(),
            max_retries: 2,
            retry_timeout: Duration::from_secs(15),
        };
        let bucket = AmazonS3Builder::new()
            .with_endpoint(&config.endpoint)
            .with_bucket_name(&config.bucket)
            .with_region(config.region.as_deref().unwrap_or(DEFAULT_REGION))
            .with_access_key_id(&credentials.id)
            .with_secret_access_key(&credentials.secret)
            .with_client_options(client)
            .with_retry(retry)
            .build()
            .map_err(|e| format!("blobs: {e}"))?;
        let bucket: Arc<dyn ObjectStore> = Arc::new(bucket);
        let store: Arc<dyn ObjectStore> = match config.prefix.as_deref() {
            Some(prefix) => Arc::new(PrefixStore::new(bucket, prefix)),
            None => bucket,
        };
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("moor-blobs")
            .enable_all()
            .build()
            .map_err(|e| format!("blobs: cannot start the client runtime: {e}"))?;
        Ok(Self {
            store,
            runtime,
            at: format!("{}/{}", config.endpoint, config.bucket),
        })
    }

    /// The blob error for a failed request, naming the store.
    fn failed(&self, e: object_store::Error) -> BlobError {
        match e {
            object_store::Error::NotFound { .. } => BlobError::Missing,
            e => BlobError::Store(format!("blob store {}: {e}", self.at)),
        }
    }

    /// Uploads `file` in parts, so its size never decides what moor holds in memory.
    fn put_parts(&self, path: &ObjectPath, file: &mut File) -> Result<(), BlobError> {
        self.runtime.block_on(async {
            let upload = self
                .store
                .put_multipart(path)
                .await
                .map_err(|e| self.failed(e))?;
            let mut out = WriteMultipart::new_with_chunk_size(upload, PART);
            let mut buf = vec![0u8; CHUNK];
            let copied = loop {
                let piece = match file.read(&mut buf) {
                    Ok(n) => buf.get(..n).filter(|p| !p.is_empty()),
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(e) => break Err(BlobError::Store(format!("a staged file: {e}"))),
                };
                let Some(piece) = piece else {
                    break Ok(());
                };
                if let Err(e) = out.wait_for_capacity(PARTS_IN_FLIGHT).await {
                    break Err(self.failed(e));
                }
                out.write(piece);
            };
            match copied {
                Ok(()) => out.finish().await.map(drop).map_err(|e| self.failed(e)),
                Err(e) => {
                    // Parts of an unfinished upload stay billed until told to go.
                    drop(out.abort().await);
                    Err(e)
                }
            }
        })
    }
}

impl Blobs for ObjectBlobs {
    fn put(&self, key: &BlobKey, bytes: &[u8]) -> Result<(), BlobError> {
        let path = ObjectPath::from(key.as_str());
        self.runtime
            .block_on(self.store.put(&path, bytes.to_vec().into()))
            .map(drop)
            .map_err(|e| self.failed(e))
    }

    fn put_file(&self, key: &BlobKey, from: &Path) -> Result<(), BlobError> {
        let unread = |e: io::Error| BlobError::Store(format!("{}: {}", from.display(), e.kind()));
        let mut file = File::open(from).map_err(unread)?;
        let size = file.metadata().map_err(unread)?.len();
        if size <= PART as u64 {
            let mut bytes = Vec::with_capacity(PART.min(usize::try_from(size).unwrap_or(PART)));
            file.read_to_end(&mut bytes).map_err(unread)?;
            self.put(key, &bytes)?;
        } else {
            self.put_parts(&ObjectPath::from(key.as_str()), &mut file)?;
        }
        fs::remove_file(from).map_err(unread)
    }

    fn size(&self, key: &BlobKey) -> Result<u64, BlobError> {
        let path = ObjectPath::from(key.as_str());
        self.runtime
            .block_on(self.store.head(&path))
            .map(|meta| meta.size)
            .map_err(|e| self.failed(e))
    }

    fn read(
        &self,
        key: &BlobKey,
        offset: u64,
        len: u64,
        sink: &mut dyn FnMut(&[u8]) -> io::Result<()>,
    ) -> Result<(), BlobError> {
        if len == 0 {
            return Ok(());
        }
        let path = ObjectPath::from(key.as_str());
        let options = GetOptions {
            range: Some(GetRange::Bounded(offset..offset + len)),
            ..GetOptions::default()
        };
        let got = self
            .runtime
            .block_on(self.store.get_opts(&path, options))
            .map_err(|e| self.failed(e))?;
        let mut pieces = got.into_stream();
        while let Some(piece) = self.runtime.block_on(pieces.next()) {
            sink(&piece.map_err(|e| self.failed(e))?).map_err(BlobError::Sink)?;
        }
        Ok(())
    }

    fn locate(&self, key: &BlobKey) -> String {
        key.as_str().to_owned()
    }

    fn name(&self) -> &'static str {
        "s3"
    }
}
