//! Route credential lookup that reports failures to the owner's stderr.

use std::sync::PoisonError;

use moor_core::proxy_gate::RouteId;
use moor_core::Secret;

use super::super::resolver::Resolver;
use super::super::upstream::{CredError, RouteCredential};
use super::Sink;

/// A [`Resolver`] that writes one diagnosable line per failed lookup.
///
/// The relay only sees an opaque [`CredError`]; the line names the config
/// source and the route. It never contains token bytes.
pub(super) struct Logged {
    /// Resolves the credentials.
    inner: Resolver,
    /// Where failure lines go.
    sink: Sink,
}

impl Logged {
    /// Wraps `inner`, reporting its failures to `sink`.
    pub(super) const fn new(inner: Resolver, sink: Sink) -> Self {
        Self { inner, sink }
    }
}

impl RouteCredential for Logged {
    /// Passes a resolved secret through; on failure writes one line, then
    /// returns the opaque error.
    fn credential(&self, route: &RouteId) -> Result<Secret, CredError> {
        self.inner.resolve(route).map_err(|e| {
            let _ = writeln!(
                self.sink.lock().unwrap_or_else(PoisonError::into_inner),
                "moor proxy: route {}: credential unavailable: {e}",
                route.as_str()
            );
            CredError
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use moor_core::seam::clock::{Clock, Ts};
    use moor_core::seam::forge::{ForgeError, ForgeRepo};

    use super::*;
    use crate::testing::clock::FakeClock;
    use crate::testing::forge::FakeForge;

    /// Concrete buffer plus the same buffer as a [`Sink`].
    fn sink() -> (Arc<Mutex<Vec<u8>>>, Sink) {
        let buf = Arc::new(Mutex::new(Vec::new()));
        let dynamic: Sink = buf.clone();
        (buf, dynamic)
    }

    /// Everything written to `buf`, as text.
    fn written(buf: &Arc<Mutex<Vec<u8>>>) -> String {
        String::from_utf8(buf.lock().unwrap().clone()).unwrap()
    }

    /// A logged resolver over `forge` and a model source returning `model`.
    fn logged(
        model: Result<&'static [u8], &'static str>,
        forge: FakeForge,
        repo: Option<ForgeRepo>,
        sink: Sink,
    ) -> Logged {
        let source: crate::proxy::resolver::ModelToken = Arc::new(move || match model {
            Ok(bytes) => Ok(Secret::new(bytes.to_vec())),
            Err(reason) => Err(reason.to_owned()),
        });
        let clock = Arc::new(FakeClock::new(Ts::from_millis(0))) as Arc<dyn Clock>;
        Logged::new(Resolver::new(source, Arc::new(forge), repo, clock), sink)
    }

    /// A failing model token names its source and the `model` route.
    #[test]
    fn model_failure_names_source_and_route() {
        let (buf, sink) = sink();
        let l = logged(Err("env X is not set"), FakeForge::new(), None, sink);
        assert!(l.credential(&RouteId::new("model")).is_err());
        assert_eq!(
            written(&buf),
            "moor proxy: route model: credential unavailable: \
             credentials.CLAUDE_CODE_OAUTH_TOKEN: env X is not set\n"
        );
    }

    /// A failing forge token names its source and the `forge.api` route.
    #[test]
    fn forge_failure_names_source_and_route() {
        let repo = ForgeRepo::new("acme", "widgets");
        let forge = FakeForge::new()
            .with_token_error(repo.clone(), ForgeError::Network("unreachable".to_owned()));
        let (buf, sink) = sink();
        let l = logged(Ok(b"m"), forge, Some(repo), sink);
        assert!(l.credential(&RouteId::new("forge.api")).is_err());
        let out = written(&buf);
        assert_eq!(out.lines().count(), 1);
        assert!(out.starts_with("moor proxy: route forge.api: credential unavailable: "));
        assert!(out.contains("github.you_token_cmd"));
    }

    /// A resolved secret passes through and nothing is written.
    #[test]
    fn success_writes_nothing_and_never_leaks_the_secret() {
        let (buf, sink) = sink();
        let l = logged(Ok(b"sk-SECRET-123"), FakeForge::new(), None, sink);
        let got = l.credential(&RouteId::new("model")).unwrap();
        assert_eq!(got.expose(), b"sk-SECRET-123");
        let bytes = buf.lock().unwrap().clone();
        assert!(bytes.is_empty());
        assert!(!written(&buf).contains("sk-SECRET-123"));
    }
}
