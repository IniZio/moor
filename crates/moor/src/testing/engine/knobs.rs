//! What a test scripts on the fake engine for transcripts.

use std::sync::Arc;

use moor_core::seam::engine::EngineError;

use super::FakeEngine;

impl FakeEngine {
    /// Every transcript request writes `text` to its destination.
    pub(crate) fn transcript_text(self, text: &str) -> Self {
        self.st().transcript_text = Some(text.to_owned());
        self
    }

    /// The next transcript request without a queued text of its own writes `text`, ahead of the fixed one.
    pub(crate) fn then_transcript_text(self, text: &str) -> Self {
        self.st().later_texts.push_back(text.to_owned());
        self
    }

    /// The next transcript request fails once with `e`.
    pub(crate) fn fail_transcript(self, e: EngineError) -> Self {
        self.st().fail_transcript = Some(e);
        self
    }

    /// The next transcript request fails once with `e`, set on an Engine already shared.
    pub(crate) fn fail_next_transcript(&self, e: EngineError) {
        self.st().fail_transcript = Some(e);
    }

    /// Runs `f` inside every transcript request, before it is served.
    pub(crate) fn on_transcript(&self, f: impl Fn() + Send + Sync + 'static) {
        self.st().on_transcript = Some(Arc::new(f));
    }
}
