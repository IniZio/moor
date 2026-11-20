//! The agent's handle on its Box provider, erased behind one trait object.

use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;

use moor_client::link::boxes::{BoxOffer, BoxReport};
use moor_client::link::BoxMemory;

use super::docker::Docker;
#[cfg(any(test, feature = "fake-boxes"))]
use super::FakeProvider;
use super::{BoxProvider, Boxes, Images, Wire};

/// A boxed future, so [`Serving`] can be held as a trait object.
type Later<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// What the rest of the agent asks of the Boxes of one provider.
trait Serving: Send + Sync {
    /// What the machine offers.
    fn offer(&self) -> BoxOffer;
    /// Whether its Boxes keep cache volumes.
    fn keeps_caches(&self) -> bool;
    /// Deletes the cache volumes no Box has used for too long.
    fn expire_caches(self: Arc<Self>) -> Later<'static, ()>;
    /// Serves the one request on `stream`.
    fn serve(self: Arc<Self>, stream: Box<dyn Wire>) -> Later<'static, ()>;
    /// Every Box's health.
    fn reports(&self) -> Later<'_, Vec<BoxReport>>;
    /// The host memory each Box holds.
    fn memory(&self) -> Later<'_, Vec<BoxMemory>>;
}

impl<P: BoxProvider> Serving for Boxes<P> {
    fn offer(&self) -> BoxOffer {
        Self::offer(self)
    }

    fn keeps_caches(&self) -> bool {
        self.provider.keeps_caches()
    }

    fn expire_caches(self: Arc<Self>) -> Later<'static, ()> {
        Box::pin(async move { self.provider.expire_caches().await })
    }

    fn serve(self: Arc<Self>, stream: Box<dyn Wire>) -> Later<'static, ()> {
        Box::pin(async move { Self::serve(&self, stream).await })
    }

    fn reports(&self) -> Later<'_, Vec<BoxReport>> {
        Box::pin(Self::reports(self))
    }

    fn memory(&self) -> Later<'_, Vec<BoxMemory>> {
        Box::pin(Self::memory(self))
    }
}

/// How this machine provides Boxes, as the rest of the agent holds it.
#[derive(Clone)]
pub struct Host(Arc<dyn Serving>);

impl Host {
    /// The host named `kind`, keeping its state under `home`; `docker` runs the release's images.
    ///
    /// # Errors
    /// No provider of this build is called `kind`.
    pub fn named(kind: &str, home: &Path) -> Result<Self, String> {
        match kind {
            "docker" => Ok(Self::docker(Images::default(), home)),
            #[cfg(any(test, feature = "fake-boxes"))]
            "fake" => Ok(Self(Arc::new(Boxes::new(
                FakeProvider::new(home.join("fake-boxes")),
                home,
            )))),
            other => Err(format!(
                "this build provides no Boxes as {other:?}; it has {}",
                providers()
            )),
        }
    }

    /// The host that makes Boxes of Docker containers from `images`, keeping its state under
    /// `home`.
    #[must_use]
    pub fn docker(images: Images, home: &Path) -> Self {
        Self(Arc::new(Boxes::new(
            Docker::new(images, None)
                .kept_in(home)
                .rootless(moor_client::rootless::detect()),
            home,
        )))
    }

    /// What the machine offers.
    #[must_use]
    pub fn offer(&self) -> BoxOffer {
        self.0.offer()
    }

    /// Whether its Boxes keep cache volumes across Boxes.
    #[must_use]
    pub fn keeps_caches(&self) -> bool {
        self.0.keeps_caches()
    }

    /// Deletes the cache volumes no Box has used for too long.
    pub(crate) async fn expire_caches(&self) {
        Arc::clone(&self.0).expire_caches().await;
    }

    /// Serves the one request on `stream`.
    pub(crate) async fn serve<S: Wire>(&self, stream: S) {
        Arc::clone(&self.0).serve(Box::new(stream)).await;
    }

    /// Every Box's health.
    pub(crate) async fn reports(&self) -> Vec<BoxReport> {
        self.0.reports().await
    }

    /// The host memory each Box holds.
    pub(crate) async fn memory(&self) -> Vec<BoxMemory> {
        self.0.memory().await
    }
}

/// The providers this build has, for an error.
const fn providers() -> &'static str {
    if cfg!(any(test, feature = "fake-boxes")) {
        "docker, fake"
    } else {
        "docker"
    }
}
