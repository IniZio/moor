//! The cache volumes of a Box as Docker named volumes.
//!
//! The controller's plan names each volume by a key that already holds the owner and the repo, so
//! a canonical volume `moor-cache-<key>-<n>` (one per path) is kept for that owner and repo across
//! Boxes and is never removed with a Box. A package cache is mounted from it directly, shared by the
//! repo's Boxes that run at once. Anything two live Boxes must not share, such as the work tree, the
//! container stores or a volume with restore steps, is copied: each Box gets a private volume filled
//! from the canonical one on open, and saves it back there when it closes.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};

use moor_client::link::lease::{Cache, CacheVolume};
use tokio::sync::RwLock;

use super::args::{SLUG_LABEL, WORKDIR};
use super::cli::Cli;
use crate::boxes::provider::{Notes, ProvideError};
use crate::boxes::Slug;

/// The Box user's home in both places.
const HOME: &str = "/home/agent";
/// The label of a canonical or private cache volume: its key.
pub(super) const KEY_LABEL: &str = "moor.cache";
/// The label of a cache volume that is a Box's private copy of a canonical one: the canonical name.
const COPY_LABEL: &str = "moor.cache.of";
/// The label of the marker volume that says a private copy finished.
const MARK_LABEL: &str = "moor.cache.mark";
/// The length of a repo scope's hash; a group longer than it carries the suffix of a work tree, an agent
/// directory or a volume with restore steps.
const SCOPE_LEN: usize = 16;
/// The most paths of one Box the agent mounts.
const MOST_MOUNTS: usize = 64;

/// Paths of tools whose caches take concurrent writers; every other path is copied per Box.
const SHARED: [&str; 17] = [
    "~/.cache",
    "~/.local/share/pnpm/store",
    "~/.bun/install/cache",
    "~/.gradle/caches",
    "~/.gradle/wrapper",
    "~/.pub-cache",
    "~/.cargo/registry",
    "~/.cargo/git",
    "~/.rustup",
    "~/go/pkg/mod",
    "~/.m2/repository",
    "~/.cocoapods/repos",
    "~/.bundle/cache",
    "~/.local/share/gem",
    "~/.android/cache",
    "~/.android/build-cache",
    "/opt/android-sdk",
];

/// One cache volume mounted at one path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Mount {
    /// The canonical volume.
    pub(super) canonical: String,
    /// The Box's private copy of it; none when the Box mounts the canonical volume itself.
    pub(super) private: Option<String>,
    /// The absolute path in the Box.
    pub(super) path: String,
}

impl Mount {
    /// The volume the Box mounts.
    pub(super) fn volume(&self) -> &str {
        self.private.as_deref().unwrap_or(&self.canonical)
    }

    /// Whether both places mount it: what is below the work tree both places share.
    fn both(&self) -> bool {
        self.path.starts_with(&format!("{WORKDIR}/"))
    }
}

/// Whether a group's volumes may be mounted by live Boxes at once: a tool cache with no restore steps
/// and none of the work tree, the agent or a container store.
fn shared(v: &CacheVolume) -> bool {
    let kept_apart = v.group.len() > SCOPE_LEN;
    !kept_apart && !v.paths.is_empty() && v.paths.iter().all(|p| SHARED.contains(&p.as_str()))
}

/// `path` as an absolute path in the Box, or why the agent refuses it.
fn absolute(path: &str) -> Result<String, ProvideError> {
    let abs = path
        .strip_prefix("~/")
        .map_or_else(|| path.to_owned(), |rest| format!("{HOME}/{rest}"));
    let plain = |c: char| c.is_ascii_alphanumeric() || "._-+@/".contains(c);
    let ok = abs.starts_with('/')
        && !abs.split('/').any(|p| p == ".." || p == ".")
        && !abs.contains("//")
        && abs.chars().all(plain);
    if ok {
        Ok(abs)
    } else {
        Err(ProvideError::Failed(format!(
            "the cache path {path:?} is not one the machine mounts"
        )))
    }
}

/// A volume key or name part: lowercase letters, digits and `-`.
fn plain(text: &str) -> bool {
    !text.is_empty()
        && text
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// The mounts `cache` asks of Box `slug`.
///
/// # Errors
/// A key, name or path the machine does not mount.
pub(super) fn mounts(slug: &Slug, cache: Option<&Cache>) -> Result<Vec<Mount>, ProvideError> {
    let Some(cache) = cache else {
        return Ok(Vec::new());
    };
    let mut all = Vec::new();
    for v in &cache.volumes {
        if !plain(&v.key) || !plain(&v.name) {
            return Err(ProvideError::Failed(format!(
                "the cache volume {:?} has a name the machine refuses",
                v.key
            )));
        }
        let copy = !shared(v);
        for (n, path) in v.paths.iter().enumerate() {
            all.push(Mount {
                canonical: format!("moor-cache-{}-{n}", v.key),
                private: copy.then(|| format!("{}-cache-{}-{n}", slug.as_str(), v.name)),
                path: absolute(path)?,
            });
        }
    }
    let mut seen: Vec<&str> = all.iter().map(|m| m.path.as_str()).collect();
    seen.sort_unstable();
    seen.dedup();
    if all.len() > MOST_MOUNTS || seen.len() != all.len() {
        return Err(ProvideError::Failed(
            "the cache volumes repeat a path or are too many".to_owned(),
        ));
    }
    Ok(all)
}

/// The `docker run` arguments that mount `mounts` in the agent place (the shared work tree
/// only) or the work place (all), and tell the work init which directories to hand to the Box user.
pub(super) fn args(mounts: &[Mount], work: bool) -> Vec<String> {
    let mut a = Vec::new();
    let mut dirs = Vec::new();
    for m in mounts.iter().filter(|m| work || m.both()) {
        a.push("-v".to_owned());
        a.push(format!("{}:{}", m.volume(), m.path));
        dirs.push(m.path.as_str());
    }
    if work && !dirs.is_empty() {
        a.push("-e".to_owned());
        a.push(format!("MOOR_CACHE_DIRS={}", dirs.join(":")));
    }
    a
}

/// What the provider keeps about cache volumes: one lock per canonical volume, a save writing it
/// alone and copies in reading it together, and when each was last used.
#[derive(Default)]
pub(super) struct Caches {
    /// The locks.
    locks: Mutex<BTreeMap<String, Arc<RwLock<()>>>>,
    /// When each canonical volume was last used.
    pub(super) uses: uses::Uses,
    /// Whether an expiry pass is running.
    pub(super) sweeping: std::sync::atomic::AtomicBool,
}

impl Caches {
    /// The lock of `canonical`.
    pub(super) fn of(&self, canonical: &str) -> Arc<RwLock<()>> {
        let mut held = self.locks.lock().unwrap_or_else(PoisonError::into_inner);
        Arc::clone(held.entry(canonical.to_owned()).or_default())
    }
}

/// Runs `script` as root in a throwaway container of `image` with `from` read-only at `/from` and `to`
/// at `/to`.
async fn copy(image: &str, from: &str, to: &str, script: &str) -> Result<String, ProvideError> {
    Cli::ok([
        "run",
        "--rm",
        "-u",
        "0",
        "--entrypoint",
        "sh",
        "-v",
        &format!("{from}:/from:ro"),
        "-v",
        &format!("{to}:/to"),
        image,
        "-c",
        script,
    ])
    .await
}

/// Copies everything of `/from` to `/to`, keeping owners and times; says `empty` when there is none.
const COPY_IN: &str =
    "[ -n \"$(ls -A /from)\" ] || { echo empty; exit 0; }; rsync -a /from/ /to/ && echo copied";
/// As [`COPY_IN`] and also deletes what `/to` holds that `/from` does not.
const SAVE: &str = "[ -n \"$(ls -A /from)\" ] || { echo empty; exit 0; }; rsync -a --delete /from/ /to/ && echo saved";

/// Makes volume `name` with `labels`; one that exists stays as it is.
async fn create(name: &str, labels: &[String]) -> Result<(), ProvideError> {
    let mut a = vec!["volume".to_owned(), "create".to_owned()];
    for l in labels {
        a.push("--label".to_owned());
        a.push(l.clone());
    }
    a.push(name.to_owned());
    Cli::ok(a).await.map(drop)
}

/// The key a canonical volume name stands for.
fn key_of(canonical: &str) -> &str {
    canonical.strip_prefix("moor-cache-").unwrap_or(canonical)
}

/// Whether `private` is a finished copy; one a crash cut short is removed so it is made again.
async fn settle(private: &str, mark: &str) -> Result<bool, ProvideError> {
    let there = Cli::passes(["volume", "inspect", private]).await;
    let marked = Cli::passes(["volume", "inspect", mark]).await;
    match standing(there, marked) {
        Private::Trust => Ok(true),
        Private::Redo => Cli::ok(["volume", "rm", "-f", private])
            .await
            .map(|_| false),
        Private::Make => Ok(false),
    }
}

/// Makes one mount's volumes ready; whether it holds a save from an earlier Box.
async fn prepare_one(
    locks: &Caches,
    image: &str,
    slug: &Slug,
    m: &Mount,
) -> Result<bool, ProvideError> {
    let key = format!("{KEY_LABEL}={}", key_of(&m.canonical));
    let had = Cli::passes(["volume", "inspect", &m.canonical]).await;
    let Some(private) = &m.private else {
        create(&m.canonical, &[key]).await?;
        return Ok(had);
    };
    let mark = marker(private);
    if settle(private, &mark).await? {
        return Ok(true);
    }
    fill(locks, image, slug, m, private, had).await
}

/// Makes the private volume and fills it from the canonical one, then marks it finished. Whether it
/// holds a save.
async fn fill(
    locks: &Caches,
    image: &str,
    slug: &Slug,
    m: &Mount,
    private: &str,
    had: bool,
) -> Result<bool, ProvideError> {
    let key = format!("{KEY_LABEL}={}", key_of(&m.canonical));
    let mark = marker(private);
    let labels = [
        format!("{SLUG_LABEL}={}", slug.as_str()),
        key.clone(),
        format!("{COPY_LABEL}={}", m.canonical),
    ];
    create(private, &labels).await?;
    create(&m.canonical, &[key]).await?;
    let marks = [
        format!("{SLUG_LABEL}={}", slug.as_str()),
        format!("{MARK_LABEL}=1"),
    ];
    if !had {
        create(&mark, &marks).await?;
        return Ok(false);
    }
    let lock = locks.of(&m.canonical);
    let _read = lock.read().await;
    match copy(image, &m.canonical, private, COPY_IN).await {
        Ok(said) => {
            create(&mark, &marks).await?;
            Ok(said.ends_with("copied"))
        }
        Err(e) => {
            drop(Cli::ok(["volume", "rm", "-f", private]).await);
            Err(e)
        }
    }
}

/// Makes the volumes of `mounts` ready for Box `slug`: each canonical volume exists, and each
/// private one that is new is filled from the canonical volume.
pub(super) async fn prepare(
    locks: &Caches,
    image: &str,
    slug: &Slug,
    mounts: &[Mount],
    note: &Notes,
) -> Result<(), ProvideError> {
    locks
        .uses
        .touch(mounts.iter().map(|m| m.canonical.as_str()));
    let mut warm = 0;
    for m in mounts {
        if prepare_one(locks, image, slug, m).await? {
            warm += 1;
        }
    }
    if !mounts.is_empty() {
        note.say(&format!(
            "cache volumes: {warm} warm, {} cold",
            mounts.len() - warm
        ));
    }
    Ok(())
}

/// Saves each private cache volume of Box `slug` into its canonical volume, skipping a volume that
/// is empty so a Box that never set up cannot wipe a good save. The caller has stopped the Box.
/// A failed save is the last error; the others still run.
pub(super) async fn save(locks: &Caches, image: &str, slug: &Slug) -> Result<(), ProvideError> {
    let format = format!("{{{{.Name}}}} {{{{.Label \"{COPY_LABEL}\"}}}}");
    let listed = Cli::ok([
        "volume",
        "ls",
        "--filter",
        &format!("label={SLUG_LABEL}={}", slug.as_str()),
        "--filter",
        &format!("label={COPY_LABEL}"),
        "--format",
        &format,
    ])
    .await?;
    let mut last = Ok(());
    for line in listed.lines() {
        let Some((private, canonical)) = line.split_once(' ') else {
            continue;
        };
        if !canonical.starts_with("moor-cache-")
            || !Cli::passes(["volume", "inspect", &marker(private)]).await
        {
            continue;
        }
        let lock = locks.of(canonical);
        let _write = lock.write().await;
        let key = format!("{KEY_LABEL}={}", key_of(canonical));
        let saved = match create(canonical, &[key]).await {
            Ok(()) => copy(image, private, canonical, SAVE).await.map(drop),
            Err(e) => Err(e),
        };
        match saved {
            Ok(()) => locks.uses.touch([canonical]),
            Err(e) => last = Err(e),
        }
    }
    last
}

/// Removes the private cache volumes of Box `slug`; the canonical volumes stay.
pub(super) async fn drop_private(slug: &Slug) -> Result<(), ProvideError> {
    for label in [COPY_LABEL, MARK_LABEL] {
        let listed = Cli::ok([
            "volume",
            "ls",
            "-q",
            "--filter",
            &format!("label={SLUG_LABEL}={}", slug.as_str()),
            "--filter",
            &format!("label={label}"),
        ])
        .await?;
        for name in listed.lines().filter(|n| n.starts_with(slug.as_str())) {
            Cli::ok(["volume", "rm", "-f", name]).await?;
        }
    }
    Ok(())
}

mod expire;
pub use expire::parse_age;
pub(super) use expire::{pass as expire, DEFAULT_MAX_AGE};
mod marker;
mod uses;
use marker::{marker, standing, Private};

#[cfg(test)]
mod tests;
