//! The agent's handle on its Box provider, erased behind one trait object.

use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;

use moor_client::link::boxes::{BoxOffer, BoxReport};

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
    /// Serves the one request on `stream`.
    fn serve(self: Arc<Self>, stream: Box<dyn Wire>) -> Later<'static, ()>;
    /// Every Box's health.
    fn reports(&self) -> Later<'_, Vec<BoxReport>>;
}

impl<P: BoxProvider> Serving for Boxes<P> {
    fn offer(&self) -> BoxOffer {
        Self::offer(self)
    }

    fn serve(self: Arc<Self>, stream: Box<dyn Wire>) -> Later<'static, ()> {
        Box::pin(async move { Self::serve(&self, stream).await })
    }

    fn reports(&self) -> Later<'_, Vec<BoxReport>> {
        Box::pin(Self::reports(self))
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
        Self(Arc::new(Boxes::new(Docker::new(images, None), home)))
    }

    /// What the machine offers.
    #[must_use]
    pub fn offer(&self) -> BoxOffer {
        self.0.offer()
    }

    /// Serves the one request on `stream`.
    pub(crate) async fn serve<S: Wire>(&self, stream: S) {
        Arc::clone(&self.0).serve(Box::new(stream)).await;
    }

    /// Every Box's health.
    pub(crate) async fn reports(&self) -> Vec<BoxReport> {
        self.0.reports().await
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
