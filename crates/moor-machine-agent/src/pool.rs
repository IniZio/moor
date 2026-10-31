//! A pool host: boots, pauses and deletes VM leases with the host's own crabbox, and carries
//! dials to a lease's sshd. The provider is the agent's own; a request names only the slug.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use moor_client::link::lease::{Answer, Boot, Lease, Verb};
use moor_client::link::ops::{Op, Reply};
use moor_client::link::Offer;
use moor_core::box_key::parse_pool_slug;
use tokio::process::Command;

pub use reach::{carry, dial, splice, Sshd};

mod cache;
mod reach;

/// How long a VM may take to boot.
const BOOT: Duration = Duration::from_mins(15);
/// How long any other crabbox call may take.
const CALL: Duration = Duration::from_mins(2);
/// The exit code of `crabbox stop` for a lease that is already gone; the external provider
/// exits 1 instead, saying the lease is not found.
const GONE: i32 = 4;
/// Bytes of crabbox's stderr an error quotes, from the end.
const TAIL: usize = 2048;

/// The pool this agent serves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pool {
    /// The crabbox provider every lease uses.
    pub provider: String,
    /// The most `moor-*` leases a warmup may leave on the host.
    pub max: Option<usize>,
    /// The disk, in GB, reported as set aside for caches; nothing enforces it.
    pub cache_gb: Option<u32>,
    /// The crabbox program.
    pub crabbox: PathBuf,
}

/// What this agent tells the server it offers; none when it serves no pool.
///
/// The features are empty when crabbox cannot say.
pub async fn offer(machine: &crate::Machine) -> Option<Offer> {
    let pool = machine.pool.as_ref()?;
    Some(Offer {
        max_boxes: pool.max.and_then(|m| u32::try_from(m).ok()),
        cache_gb: pool.cache_gb,
        features: features(pool).await,
    })
}

/// The features `crabbox providers describe` lists for the pool's provider.
async fn features(pool: &Pool) -> Vec<String> {
    let mut cmd = Command::new(&pool.crabbox);
    cmd.args(["providers", "describe", &pool.provider, "--json"])
        .stdin(Stdio::null())
        .kill_on_drop(true);
    let Ok(Ok(out)) = tokio::time::timeout(CALL, cmd.output()).await else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    let Ok(described) = serde_json::from_slice::<serde_json::Value>(&out.stdout) else {
        return Vec::new();
    };
    let mut features: Vec<String> = described
        .pointer("/capabilities/features")
        .and_then(|f| serde_json::from_value(f.clone()).ok())
        .unwrap_or_default();
    features.sort();
    features
}

/// Does `verb` to the lease `slug`, or lists the leases, under `home`; an authorize lets `key`
/// in, a warmup boots with `boot`.
pub async fn lease(
    home: &Path,
    pool: &Pool,
    verb: Verb,
    slug: Option<&str>,
    key: Option<&str>,
    boot: &Boot,
) -> Reply {
    match answer(home, pool, verb, slug, key, boot).await {
        Ok(answer) => Reply::Leased(answer),
        Err(reason) => Reply::Refused { reason },
    }
}

/// Serves a lease op; anything else is refused.
pub async fn lease_op(home: &Path, pool: &Pool, op: Op) -> Reply {
    match op {
        Op::Lease {
            verb,
            slug,
            key,
            cache,
            class,
        } => {
            let boot = Boot { cache, class };
            lease(home, pool, verb, slug.as_deref(), key.as_deref(), &boot).await
        }
        _ => Reply::Refused {
            reason: "not a lease".to_owned(),
        },
    }
}

/// What `verb` came to.
async fn answer(
    home: &Path,
    pool: &Pool,
    verb: Verb,
    slug: Option<&str>,
    key: Option<&str>,
    boot: &Boot,
) -> Result<Answer, String> {
    if boot.cache.is_some() && verb != Verb::Warmup {
        return Err("only a warmup mounts a cache".to_owned());
    }
    if boot.class.is_some() && verb != Verb::Warmup {
        return Err("only a warmup boots a machine class".to_owned());
    }
    match (verb, slug.map(checked).transpose()?) {
        (Verb::List, None) => Ok(Answer::Leases {
            leases: list(home, pool).await?,
        }),
        (Verb::List, Some(_)) | (_, None) => {
            Err("list names no slug; every other verb names one".to_owned())
        }
        (verb, Some(slug)) => {
            act(
                &Crabbox {
                    home,
                    pool,
                    slug: Some(slug),
                },
                verb,
                slug,
                key,
                boot,
            )
            .await
        }
    }
}

/// Does `verb` to lease `slug`.
async fn act(
    lease: &Crabbox<'_>,
    verb: Verb,
    slug: &str,
    key: Option<&str>,
    boot: &Boot,
) -> Result<Answer, String> {
    let ran = match verb {
        Verb::Authorize => {
            let key = key.ok_or("authorize names no key")?;
            return reach::authorize(lease, key).await;
        }
        Verb::Warmup => warmup(lease, slug, boot).await,
        Verb::Info => return reach::info(lease).await,
        Verb::Pause => lease.run("pause", "--id", CALL, &[0]).await,
        Verb::Resume => lease.run("resume", "--id", CALL, &[0]).await,
        Verb::Stop => {
            let gone = lease.run("stop", "--id", CALL, &[0, GONE]).await;
            let _ = tokio::fs::remove_dir_all(box_dir(lease.home, slug)).await;
            gone
        }
        Verb::List => return Err("list names no slug".to_owned()),
    };
    match ran {
        Ok(_) => Ok(Answer::Done),
        Err(e) if verb == Verb::Pause && e.contains("does not support pause") => {
            Ok(Answer::Unsupported)
        }
        Err(e) if verb == Verb::Stop && e.contains(&format!("lease {slug} not found")) => {
            Ok(Answer::Done)
        }
        Err(e) => Err(e),
    }
}

/// Boots lease `slug` with `boot`; a bad cache or class stops it before crabbox runs.
async fn warmup(lease: &Crabbox<'_>, slug: &str, boot: &Boot) -> Result<String, String> {
    let env = boot.cache.as_ref().map(cache::env).transpose()?;
    let class = boot.class.as_deref().map(checked_class).transpose()?;
    let flags = class.map_or_else(|| vec!["--slug"], |c| vec!["--class", c, "--slug"]);
    room(lease.home, lease.pool, slug).await?;
    let env = env.unwrap_or_default();
    lease.run_with("warmup", &flags, BOOT, &[0], &env).await
}

/// `slug` itself, when it is a well-formed `moor-<controller>-<id>` slug.
fn checked(slug: &str) -> Result<&str, String> {
    parse_pool_slug(slug)
        .map(|_| slug)
        .ok_or_else(|| format!("{slug:?} is not a moor-<controller>-<id> slug"))
}

/// `class` itself, when it is a token crabbox can take as a machine class: lowercase letters,
/// digits, `.`, `_` and `-`, starting with a letter or digit, at most 32 bytes.
fn checked_class(class: &str) -> Result<&str, String> {
    let token = class
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-'));
    let leads = class
        .bytes()
        .next()
        .is_some_and(|b| b.is_ascii_alphanumeric());
    if token && leads && class.len() <= 32 {
        Ok(class)
    } else {
        Err(format!("{class:?} is not a machine class"))
    }
}

/// Ok unless booting `slug` would pass the pool's most leases.
async fn room(home: &Path, pool: &Pool, slug: &str) -> Result<(), String> {
    let Some(max) = pool.max else {
        return Ok(());
    };
    let others = list(home, pool)
        .await?
        .iter()
        .filter(|l| l.slug != slug)
        .count();
    if others >= max {
        return Err(format!("the pool already holds {others} leases, its most"));
    }
    Ok(())
}

/// The `moor-<controller>-<id>` leases `crabbox list` shows. A lease names its slug in `labels.slug`, `slug`
/// or `name`, and its state in `state` or `status`, by provider.
pub(crate) async fn list(home: &Path, pool: &Pool) -> Result<Vec<Lease>, String> {
    let printed = Crabbox {
        home,
        pool,
        slug: None,
    }
    .run("list", "--json", CALL, &[0])
    .await?;
    let leases: Vec<serde_json::Value> = serde_json::from_str(printed.trim())
        .map_err(|e| format!("crabbox list printed no JSON list: {e}"))?;
    Ok(leases
        .iter()
        .filter_map(|l| {
            let slug = [l.pointer("/labels/slug"), l.get("slug"), l.get("name")]
                .into_iter()
                .flatten()
                .find_map(serde_json::Value::as_str)
                .filter(|s| parse_pool_slug(s).is_some())?;
            let state = [l.get("state"), l.get("status")]
                .into_iter()
                .flatten()
                .find_map(serde_json::Value::as_str)
                .unwrap_or("unknown");
            Some(Lease {
                slug: slug.to_owned(),
                state: state.to_owned(),
            })
        })
        .collect())
}

/// Where the pool keeps its leases' dirs: apart from the `moor-state` of Boxes that hold this
/// account as a machine, whose ids come from other controllers.
const POOL_DIR: &str = "moor-pool";

/// Everything the host keeps for the lease `slug`, which checked has made safe as a dir name.
fn box_dir(home: &Path, slug: &str) -> PathBuf {
    home.join(POOL_DIR).join(slug)
}

/// crabbox on this host, for one lease or, with no slug, for the pool.
struct Crabbox<'a> {
    /// The account's home.
    home: &'a Path,
    /// The pool.
    pool: &'a Pool,
    /// The lease's slug.
    slug: Option<&'a str>,
}

impl Crabbox<'_> {
    /// Runs `crabbox VERB --provider P FLAG SLUG`, or `FLAG` alone with no slug, within
    /// `within`; its stdout when it exits with a code in `ok`.
    async fn run(
        &self,
        verb: &str,
        flag: &str,
        within: Duration,
        ok: &[i32],
    ) -> Result<String, String> {
        self.run_with(verb, &[flag], within, ok, &[]).await
    }

    /// [`Self::run`] with `flags` in place of its one flag and `env` added to crabbox's.
    async fn run_with(
        &self,
        verb: &str,
        flags: &[&str],
        within: Duration,
        ok: &[i32],
        env: &[(&str, String)],
    ) -> Result<String, String> {
        let base = self.home.join(POOL_DIR);
        let mine = self.slug.map(|slug| box_dir(self.home, slug));
        // Only a warmup makes a lease's dir, so a call on a lease that is gone leaves nothing
        // behind; crabbox itself says whether the lease exists.
        let (dir, ceiling) = match mine {
            Some(mine) if verb == "warmup" || mine.join("lease").is_dir() => {
                (mine.join("lease"), mine)
            }
            _ => (base.clone(), base),
        };
        std::fs::create_dir_all(&dir).map_err(|e| format!("cannot make {}: {e}", dir.display()))?;
        let mut cmd = Command::new(&self.pool.crabbox);
        cmd.args([verb, "--provider", &self.pool.provider])
            .args(flags);
        if let Some(slug) = self.slug {
            cmd.arg(slug);
        }
        cmd.current_dir(&dir)
            .env("HOME", self.home)
            .env("GIT_CEILING_DIRECTORIES", ceiling)
            .envs(env.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::null())
            .kill_on_drop(true);
        let out = tokio::time::timeout(within, cmd.output())
            .await
            .map_err(|_| format!("crabbox {verb} ran past {}s", within.as_secs()))?
            .map_err(|e| format!("cannot run crabbox: {e}"))?;
        match out.status.code() {
            Some(code) if ok.contains(&code) => {
                Ok(String::from_utf8_lossy(&out.stdout).into_owned())
            }
            code => {
                let err = String::from_utf8_lossy(&out.stderr);
                let cut = err.len().saturating_sub(TAIL);
                let tail = err.get(cut..).unwrap_or(&err).trim();
                Err(format!("crabbox {verb} failed ({code:?}): {tail}"))
            }
        }
    }
}
