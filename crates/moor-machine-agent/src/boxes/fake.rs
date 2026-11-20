//! A provider for tests: a Box is a directory, and a place is a local `sh` that starts in the
//! Box's workdir with a home of its own and `MOOR_PLACE` naming it.

#![allow(
    clippy::unused_async_trait_impl,
    reason = "a fake provider has nothing to await"
)]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};

use moor_core::seam::driver::Place;
use tokio::process::Command;

use super::provider::{BoxProvider, Ended, Env, Notes, Observed, PlaceState, ProvideError, Spec};
use super::Slug;

/// One fake Box.
#[derive(Debug, Default, Clone)]
struct Fake {
    /// Whether its places run.
    awake: bool,
    /// Whether its work place is down while the agent place runs.
    work_down: bool,
    /// What `apply_env` last gave it.
    box_env: Env,
    /// Whether it was sealed.
    sealed: bool,
    /// MiB of the host it holds while it runs, when a test set it.
    mem_mib: Option<u64>,
}

/// Boxes made of directories under `root`.
pub(crate) struct FakeProvider {
    /// Where each Box's directory is.
    root: PathBuf,
    /// The Boxes there are.
    boxes: Mutex<BTreeMap<Slug, Fake>>,
    /// Opens finish only while they can read it.
    gate: tokio::sync::RwLock<()>,
    /// How many opens go on now, and the most that ever went on at once.
    opening: (AtomicU32, AtomicU32),
    /// What `observe` fails with, when a test set it.
    unseen: Mutex<Option<ProvideError>>,
}

impl FakeProvider {
    /// A provider keeping its Boxes under `root`.
    pub(crate) fn new(root: PathBuf) -> Self {
        Self {
            root,
            boxes: Mutex::default(),
            gate: tokio::sync::RwLock::default(),
            opening: (AtomicU32::new(0), AtomicU32::new(0)),
            unseen: Mutex::default(),
        }
    }

    /// Holds every open short of finishing until the guard drops.
    #[cfg(test)]
    pub(crate) async fn hold_opens(&self) -> tokio::sync::RwLockWriteGuard<'_, ()> {
        self.gate.write().await
    }

    /// The most opens that ever went on at once.
    #[cfg(test)]
    pub(crate) fn most_opens_at_once(&self) -> u32 {
        self.opening.1.load(Ordering::SeqCst)
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

    /// Has every later `observe` fail with `e`, as a Docker daemon that stopped answering does.
    #[cfg(test)]
    pub(crate) fn fail_observe(&self, e: ProvideError) {
        *self.unseen.lock().unwrap_or_else(PoisonError::into_inner) = Some(e);
    }

    /// Makes `slug` hold `mib` of the host while it runs.
    #[cfg(test)]
    pub(crate) fn set_mem(&self, slug: &Slug, mib: u64) {
        if let Some(fake) = self.boxes().get_mut(slug) {
            fake.mem_mib = Some(mib);
        }
    }

    /// Whether `slug` was sealed.
    #[cfg(test)]
    pub(crate) fn sealed(&self, slug: &Slug) -> bool {
        self.boxes().get(slug).is_some_and(|f| f.sealed)
    }

    /// How `slug`'s places died under it, as a `died` file in its directory says: what a
    /// `docker kill` from outside the agent leaves of a Box, for a test outside this crate. The
    /// file holds the exit code, and `oom` after it when the memory limit killed them.
    fn died(&self, slug: &Slug) -> Option<Ended> {
        let text = std::fs::read_to_string(self.root.join(slug.as_str()).join("died")).ok()?;
        let mut words = text.split_whitespace();
        Some(Ended {
            code: words.next().and_then(|c| c.parse().ok()).unwrap_or(137),
            oom: words.next() == Some("oom"),
        })
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
        let now = self.opening.0.fetch_add(1, Ordering::SeqCst) + 1;
        self.opening.1.fetch_max(now, Ordering::SeqCst);
        drop(self.gate.read().await);
        self.opening.0.fetch_sub(1, Ordering::SeqCst);
        let workdir = self.workdir(slug);
        let dir = self.root.join(slug.as_str());
        for place in [&workdir, &dir.join("agent-home"), &dir.join("work-home")] {
            std::fs::create_dir_all(place).map_err(|e| ProvideError::Failed(e.to_string()))?;
        }
        self.boxes().entry(slug.clone()).or_default().awake = true;
        Ok(workdir.to_string_lossy().into_owned())
    }

    async fn start(&self, slug: &Slug) -> Result<(), ProvideError> {
        let _ = std::fs::remove_file(self.root.join(slug.as_str()).join("died"));
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
        self.with(slug, |fake| fake.box_env = env.clone())
    }

    async fn observe(&self) -> Result<Vec<Observed>, ProvideError> {
        if let Some(e) = self
            .unseen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
        {
            return Err(e);
        }
        let up = |on: bool| if on { PlaceState::Up } else { PlaceState::Down };
        Ok(self
            .boxes()
            .iter()
            .map(|(slug, fake)| {
                let died = self.died(slug).map(PlaceState::Exited);
                let agent = died.unwrap_or_else(|| up(fake.awake));
                (
                    slug,
                    agent,
                    died.unwrap_or_else(|| up(fake.awake && !fake.work_down)),
                )
            })
            .map(|(slug, agent, work)| Observed {
                slug: slug.clone(),
                agent,
                work,
                oom_kills: 0,
                mem_mib: None,
            })
            .collect())
    }

    async fn held_mib(&self) -> BTreeMap<Slug, u64> {
        self.boxes()
            .iter()
            .filter(|(_, fake)| fake.awake)
            .filter_map(|(slug, fake)| Some((slug.clone(), fake.mem_mib?)))
            .collect()
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
            .envs(fake.box_env.vars)
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
