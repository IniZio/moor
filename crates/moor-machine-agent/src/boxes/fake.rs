//! A provider for tests: a Box is a directory, and a place is a local `sh` that starts in the
//! Box's workdir with a home of its own and `MOOR_PLACE` naming it.

#![allow(
    clippy::unused_async_trait_impl,
    reason = "a fake provider has nothing to await"
)]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, PoisonError};

use moor_core::seam::driver::Place;
use tokio::process::Command;

use super::provider::{BoxProvider, Env, Notes, Observed, PlaceState, ProvideError, Spec};
use super::Slug;

/// One fake Box.
#[derive(Debug, Default, Clone)]
struct Fake {
    /// Whether its places run.
    awake: bool,
    /// Whether its work place is down while the agent place runs.
    work_down: bool,
    /// What `apply_env` last gave it.
    env: Env,
    /// Whether it was sealed.
    sealed: bool,
}

/// Boxes made of directories under `root`.
pub(crate) struct FakeProvider {
    /// Where each Box's directory is.
    root: PathBuf,
    /// The Boxes there are.
    boxes: Mutex<BTreeMap<Slug, Fake>>,
}

impl FakeProvider {
    /// A provider keeping its Boxes under `root`.
    pub(crate) fn new(root: PathBuf) -> Self {
        Self {
            root,
            boxes: Mutex::default(),
        }
    }

    /// The Boxes there are.
    fn boxes(&self) -> MutexGuard<'_, BTreeMap<Slug, Fake>> {
        self.boxes.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// `slug`'s workdir.
    fn workdir(&self, slug: &Slug) -> PathBuf {
        self.root.join(slug.as_str()).join("workdir")
    }

    /// Takes `slug`'s work place down or brings it back, as a failure of it would.
    #[cfg(test)]
    pub(crate) fn set_work_down(&self, slug: &Slug, down: bool) {
        if let Some(fake) = self.boxes().get_mut(slug) {
            fake.work_down = down;
        }
    }

    /// Whether `slug` was sealed.
    #[cfg(test)]
    pub(crate) fn sealed(&self, slug: &Slug) -> bool {
        self.boxes().get(slug).is_some_and(|f| f.sealed)
    }

    /// Runs `change` on `slug`'s Box, if there is one.
    fn with(&self, slug: &Slug, change: impl FnOnce(&mut Fake)) -> Result<(), ProvideError> {
        self.boxes()
            .get_mut(slug)
            .map(change)
            .ok_or(ProvideError::Missing)
    }
}

impl BoxProvider for FakeProvider {
    fn provides(&self) -> &'static str {
        "fake"
    }

    async fn open(&self, slug: &Slug, _spec: &Spec, note: &Notes) -> Result<String, ProvideError> {
        note.say("making the fake Box");
        let workdir = self.workdir(slug);
        let dir = self.root.join(slug.as_str());
        for place in [&workdir, &dir.join("agent-home"), &dir.join("work-home")] {
            std::fs::create_dir_all(place).map_err(|e| ProvideError::Failed(e.to_string()))?;
        }
        self.boxes().entry(slug.clone()).or_default().awake = true;
        Ok(workdir.to_string_lossy().into_owned())
    }

    async fn start(&self, slug: &Slug) -> Result<(), ProvideError> {
        self.with(slug, |fake| fake.awake = true)
    }

    async fn stop(&self, slug: &Slug) -> Result<(), ProvideError> {
        self.with(slug, |fake| fake.awake = false)
    }

    async fn remove(&self, slug: &Slug) -> Result<(), ProvideError> {
        self.boxes().remove(slug);
        let _ = std::fs::remove_dir_all(self.root.join(slug.as_str()));
        Ok(())
    }

    async fn seal(&self, slug: &Slug) -> Result<(), ProvideError> {
        self.with(slug, |fake| fake.sealed = true)
    }

    async fn apply_env(&self, slug: &Slug, env: &Env) -> Result<(), ProvideError> {
        self.with(slug, |fake| fake.env = env.clone())
    }

    async fn observe(&self) -> Result<Vec<Observed>, ProvideError> {
        let up = |on: bool| if on { PlaceState::Up } else { PlaceState::Down };
        Ok(self
            .boxes()
            .iter()
            .map(|(slug, fake)| Observed {
                slug: slug.clone(),
                agent: up(fake.awake),
                work: up(fake.awake && !fake.work_down),
                oom_kills: 0,
                mem_mib: None,
            })
            .collect())
    }

    fn carrier(&self, slug: &Slug, place: Place, _tty: bool) -> Command {
        let fake = self.boxes().get(slug).cloned().unwrap_or_default();
        let mut sh = Command::new("/bin/sh");
        sh.current_dir(self.workdir(slug))
            .env(
                "HOME",
                self.root
                    .join(slug.as_str())
                    .join(format!("{}-home", place.as_str())),
            )
            .env("MOOR_PLACE", place.as_str())
            .envs(fake.env.vars)
            .arg("-c");
        if place == Place::Work && fake.work_down {
            // The carrier of a place that is down says so as every provider does: exit 75.
            sh.arg("exit 75");
        }
        sh
    }

    fn forward_script(&self, port: u16) -> String {
        format!(
            "exec bash -c 'exec 3<>/dev/tcp/127.0.0.1/{port} || exit 1; cat <&3 & r=$!; cat >&3; wait $r'"
        )
    }
}
