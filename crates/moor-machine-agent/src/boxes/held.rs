//! What this agent keeps per Box, at `<home>/boxes/<slug>.json`: what the controller wants of
//! it, which the provider's own state cannot say (a stopped Box was slept or it crashed).
//!
//! Only a Box's flight writes its file, by writing a temp file and renaming it over.

use std::fs;
use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::provider::{Env, Spec};
use super::Slug;

/// What the controller wants of a Box.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Want {
    /// Running.
    Awake,
    /// Stopped.
    Asleep,
}

/// One Box's held state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Held {
    /// Whether it should run.
    pub(crate) want: Want,
    /// What it was opened from.
    pub(crate) spec: Spec,
    /// The environment the controller last gave it.
    pub(crate) env: Env,
    /// It grows whenever the work place's processes were lost: the first open, and every wake.
    pub(crate) generation: u32,
    /// Whether the work place has a baseline.
    pub(crate) sealed: bool,
    /// Why the machine gave up on it, when it did.
    #[serde(default)]
    pub(crate) broken: Option<String>,
}

impl Held {
    /// A Box just opened from `spec`, before its first generation.
    pub(crate) fn new(spec: Spec) -> Self {
        Self {
            want: Want::Awake,
            spec,
            env: Env::default(),
            generation: 0,
            sealed: false,
            broken: None,
        }
    }
}

/// The directory of held files.
#[derive(Debug, Clone)]
pub(crate) struct Store {
    /// `<home>/boxes`.
    dir: PathBuf,
}

impl Store {
    /// The store under `home`.
    pub(crate) fn under(home: &std::path::Path) -> Self {
        Self {
            dir: home.join("boxes"),
        }
    }

    /// `slug`'s file.
    fn file(&self, slug: &Slug) -> PathBuf {
        self.dir.join(format!("{}.json", slug.as_str()))
    }

    /// `slug`'s held state; none when there is none.
    pub(crate) fn load(&self, slug: &Slug) -> io::Result<Option<Held>> {
        match fs::read(self.file(slug)) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map(Some)
                .map_err(io::Error::other),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Replaces `slug`'s held state with `held`.
    pub(crate) fn save(&self, slug: &Slug, held: &Held) -> io::Result<()> {
        fs::create_dir_all(&self.dir)?;
        let file = self.file(slug);
        let tmp = file.with_extension("tmp");
        fs::write(&tmp, serde_json::to_vec(held).map_err(io::Error::other)?)?;
        fs::rename(&tmp, file)
    }

    /// Forgets `slug`; none held is fine.
    pub(crate) fn forget(&self, slug: &Slug) -> io::Result<()> {
        match fs::remove_file(self.file(slug)) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }

    /// Every Box held.
    pub(crate) fn slugs(&self) -> Vec<Slug> {
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let mut slugs: Vec<Slug> = entries
            .filter_map(Result::ok)
            .filter_map(|e| {
                let name = e.file_name().into_string().ok()?;
                Slug::parse(name.strip_suffix(".json")?)
            })
            .collect();
        slugs.sort();
        slugs
    }
}
