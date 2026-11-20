//! The host memory each Box holds, for the report the agent sends the runner.

use moor_client::link::BoxMemory;

use super::held::{Held, Want};
use super::provider::BoxProvider;
use super::{Boxes, Slug};

impl<P: BoxProvider> Boxes<P> {
    /// Records in `held` what `slug` holds now, as it goes to sleep, for its wake to ask; keeps
    /// the last figure when the provider cannot measure it.
    pub(super) async fn keep_slept_mib(&self, slug: &Slug, held: &mut Held) {
        let now = self.provider.held_mib().await.remove(slug);
        held.slept_mib = now.or(held.slept_mib);
    }

    /// What each Box held holds of the host: a running one what its provider measures now, an
    /// asleep one nothing, with what it held when it went to sleep as what its wake needs. A
    /// running Box the provider cannot measure is left out.
    pub(crate) async fn memory(&self) -> Vec<BoxMemory> {
        let mut measured = self.provider.held_mib().await;
        let mut said = Vec::new();
        for slug in self.store.slugs() {
            let Ok(Some(held)) = self.store.load(&slug) else {
                continue;
            };
            let (host_memory_mib, resume_memory_mib) = match held.want {
                Want::Asleep => (0, held.slept_mib),
                Want::Awake => match measured.remove(&slug) {
                    Some(mib) => (mib, None),
                    None => continue,
                },
            };
            said.push(BoxMemory {
                slug: slug.as_str().to_owned(),
                host_memory_mib,
                resume_memory_mib,
            });
        }
        said
    }
}
