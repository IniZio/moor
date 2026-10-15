//! A scripted release download for box agent tests.

use std::sync::{Arc, Mutex, PoisonError};

use super::{Fetch, NonoRelease};

/// Answers every download with one scripted body and records the URLs asked for.
pub(crate) struct FakeFetch {
    /// What each download returns.
    body: Result<Vec<u8>, String>,
    /// The URLs asked for, in order.
    urls: Mutex<Vec<String>>,
}

impl FakeFetch {
    /// A download that returns `body`.
    pub(crate) fn new(body: Result<Vec<u8>, String>) -> Self {
        Self {
            body,
            urls: Mutex::new(Vec::new()),
        }
    }

    /// The URLs asked for so far.
    pub(crate) fn urls(&self) -> Vec<String> {
        self.urls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl Fetch for FakeFetch {
    fn get(&self, url: &str) -> Result<Vec<u8>, String> {
        self.urls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(url.to_owned());
        self.body.clone()
    }
}

/// A release whose downloads all fail, for tests that never install the tool.
pub(crate) fn offline_release() -> Arc<NonoRelease> {
    let fetch = Arc::new(FakeFetch::new(Err("offline".to_owned())));
    Arc::new(NonoRelease::new(fetch, None))
}
