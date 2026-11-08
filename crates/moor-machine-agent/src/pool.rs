//! A pool host: boots, pauses and deletes VM leases with the host's own crabbox, and carries
//! dials to a lease's sshd. The provider is the agent's own; a request names only the slug.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use moor_client::link::lease::{Answer, Boot, Lease, Verb};
use moor_client::link::ops::{Op, Reply};
use moor_client::link::{BoxMemory, Offer};
use moor_core::box_key::parse_pool_slug;
use tokio::process::Command;

pub use reach::{carry, dial, splice, Sshd};

mod cache;
mod checkpoint;
pub(crate) use checkpoint::doctor_failures;
mod crabbox;
mod flight;
mod memory;
mod reach;
mod rows;
mod sweep;
mod takeover;

pub use checkpoint::checkpoint_op;
use crabbox::Crabbox;
use memory::host_mem_mib;
pub(crate) use memory::send_reports;
use rows::{held_memory, leases};
pub(crate) use sweep::sweep;
use takeover::{adopt, stop_now};

/// How long a VM may take to boot.
const BOOT: Duration = Duration::from_mins(15);
/// How long any other crabbox call may take.
const CALL: Duration = Duration::from_mins(2);
/// The exit code of `crabbox stop` for a lease that is already gone; the external provider
/// exits 1 instead, saying the lease is not found.
const GONE: i32 = 4;
/// The variable that caps the size of crabbox's cache store, in GB.
const CACHE_CAP: &str = "CRABBOX_CACHE_STORE_MAX_GB";
/// Asks crabbox's pause to save the lease's caches.
const PAUSE_PUBLISH: &str = "CRABBOX_PAUSE_PUBLISH";
/// Bytes of crabbox's stderr an error quotes, from the end.
const TAIL: usize = 2048;

/// The pool this agent serves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pool {
    /// The crabbox provider every lease uses.
    pub provider: String,
    /// The most `moor-*` leases a warmup may leave on the host.
    pub max: Option<usize>,
    /// The memory, in MiB, kept for the host; none means the server's default.
    pub reserve_mib: Option<u64>,
    /// The disk, in GB, set aside for caches: reported to the server and handed to crabbox as
    /// `CRABBOX_CACHE_STORE_MAX_GB` on every call.
    pub cache_gb: Option<u32>,
    /// The crabbox program.
    pub crabbox: PathBuf,
}

/// What this agent tells the server it offers; none when it serves no pool.
///
/// The features are empty when crabbox cannot say.
pub async fn offer(machine: &crate::Machine) -> Option<Offer> {
    let pool = machine.pool.as_ref()?;
    let features = features(pool).await;
    let checkpoints_off = checkpoint::off(&machine.home, pool, &features).await;
    Some(Offer {
        max_boxes: pool.max.and_then(|m| u32::try_from(m).ok()),
        cache_gb: pool.cache_gb,
        features,
        checkpoints: checkpoints_off.is_none(),
        checkpoints_off,
        mem_mib: host_mem_mib(),
        cpus: std::thread::available_parallelism()
            .ok()
            .and_then(|n| u32::try_from(n.get()).ok()),
        reserve_mib: pool.reserve_mib,
    })
}

/// The features `crabbox providers describe --configured` lists for the pool's provider.
///
/// Without `--configured` crabbox leaves out what the operator's config turns on, such as
/// cache volumes. A crabbox too old for the flag fails, so the pool offers no features; the
/// first such failure is logged with crabbox's own words.
async fn features(pool: &Pool) -> Vec<String> {
    static WARNED: AtomicBool = AtomicBool::new(false);
    let mut cmd = Command::new(&pool.crabbox);
    cmd.args([
        "providers",
        "describe",
        &pool.provider,
        "--configured",
        "--json",
    ])
    .stdin(Stdio::null())
    .kill_on_drop(true);
    if let Some(gb) = pool.cache_gb {
        cmd.env(CACHE_CAP, gb.to_string());
    }
    let Ok(Ok(out)) = tokio::time::timeout(CALL, cmd.output()).await else {
        return Vec::new();
    };
    if !out.status.success() {
        if !WARNED.swap(true, Ordering::Relaxed) {
            let err = String::from_utf8_lossy(&out.stderr);
            let cut = err.len().saturating_sub(TAIL);
            let tail = err.get(cut..).unwrap_or(&err).trim();
            let _ = writeln!(
                std::io::stderr(),
                "moor-machine-agent: crabbox providers describe failed ({:?}), so this pool offers no features; \
                 an older crabbox may lack --configured: {tail}",
                out.status.code()
            );
        }
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
        Op::Checkpoint { act } => checkpoint_op(home, pool, act).await,
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
    let boots = matches!(verb, Verb::Warmup | Verb::Adopt);
    if boot.cache.is_some() && !boots {
        return Err("only a warmup mounts a cache".to_owned());
    }
    if boot.class.is_some() && !boots {
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
        Verb::Adopt => return adopt(lease, slug, boot).await,
        Verb::Cancel => {
            flight::stop(&box_dir(lease.home, slug)).await;
            return Ok(Answer::Done);
        }
        Verb::Info => return reach::info(lease).await,
        Verb::Pause => return pause(lease, &[]).await,
        Verb::PausePublish => return pause(lease, &[(PAUSE_PUBLISH, "1".to_owned())]).await,
        Verb::Resume => lease.run("resume", "--id", CALL, &[0]).await,
        Verb::Stop => {
            // A stop while the VM boots finds no lease yet, and the boot then leaves one behind.
            flight::stop(&box_dir(lease.home, slug)).await;
            stop_now(lease, slug).await
        }
        Verb::List => return Err("list names no slug".to_owned()),
    };
    match ran {
        Ok(_) => Ok(Answer::Done),
        Err(e) if verb == Verb::Stop && e.contains(&format!("lease {slug} not found")) => {
            Ok(Answer::Done)
        }
        Err(e) => Err(e),
    }
}

/// Pauses the lease with `env` added to crabbox's.
async fn pause(lease: &Crabbox<'_>, env: &[(&str, String)]) -> Result<Answer, String> {
    match lease.run_both("pause", &["--id"], CALL, &[0], env).await {
        Ok((_, said)) => {
            log_pause(lease.slug.unwrap_or_default(), &said);
            Ok(Answer::Done)
        }
        Err(e) if e.contains("does not support pause") => Ok(Answer::Unsupported),
        Err(e) => Err(e),
    }
}

/// Logs what crabbox `said` while pausing `slug`: only there can a host see whether the caches
/// were saved. It is another program's output, so only its end is kept and no control character
/// reaches the log.
fn log_pause(slug: &str, said: &str) {
    let mut cut = said.len().saturating_sub(TAIL);
    while !said.is_char_boundary(cut) {
        cut += 1;
    }
    let mut out = std::io::stderr().lock();
    for line in said.get(cut..).unwrap_or_default().lines() {
        let line: String = line
            .chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect();
        let line = line.trim();
        if !line.is_empty() {
            let _ = writeln!(out, "moor-machine-agent: pause {slug}: {line}");
        }
    }
}

/// Boots lease `slug` with `boot`; a bad cache or class stops it before crabbox runs.
async fn warmup(lease: &Crabbox<'_>, slug: &str, boot: &Boot) -> Result<String, String> {
    let env = boot.cache.as_ref().map(cache::env).transpose()?;
    let class = boot.class.as_deref().map(checked_class).transpose()?;
    let flags = class.map_or_else(|| vec!["--slug"], |c| vec!["--class", c, "--slug"]);
    room(lease.home, lease.pool, slug).await?;
    let env = env.unwrap_or_default();
    let flags: Vec<String> = flags.into_iter().map(str::to_owned).collect();
    let (home, pool, name) = (
        lease.home.to_path_buf(),
        lease.pool.clone(),
        slug.to_owned(),
    );
    flight::fly(&box_dir(lease.home, slug), async move {
        let lease = Crabbox {
            home: &home,
            pool: &pool,
            slug: Some(&name),
        };
        let env: Vec<(&str, String)> = env;
        let flags: Vec<&str> = flags.iter().map(String::as_str).collect();
        lease
            .run_with("warmup", &flags, BOOT, &[0], &env)
            .await
            .map(drop)
    })
    .await
    .map(|()| String::new())
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

/// The rows of `crabbox list --json`.
async fn rows(home: &Path, pool: &Pool) -> Result<Vec<serde_json::Value>, String> {
    let printed = Crabbox {
        home,
        pool,
        slug: None,
    }
    .run("list", "--json", CALL, &[0])
    .await?;
    serde_json::from_str(printed.trim())
        .map_err(|e| format!("crabbox list printed no JSON list: {e}"))
}

/// The `moor-<controller>-<id>` leases `crabbox list` shows. A lease names its state in `state` or
/// `status`, by provider.
pub(crate) async fn list(home: &Path, pool: &Pool) -> Result<Vec<Lease>, String> {
    Ok(leases(&rows(home, pool).await?))
}

/// The host memory each pool lease holds, from the rows whose provider reports `hostMemoryMiB`.
pub(crate) async fn box_memory(home: &Path, pool: &Pool) -> Result<Vec<BoxMemory>, String> {
    Ok(held_memory(&rows(home, pool).await?))
}

/// Where the pool keeps its leases' dirs: apart from the `moor-state` of Boxes that hold this
/// account as a machine, whose ids come from other controllers.
const POOL_DIR: &str = "moor-pool";

/// Everything the host keeps for the lease `slug`, which checked has made safe as a dir name.
fn box_dir(home: &Path, slug: &str) -> PathBuf {
    home.join(POOL_DIR).join(slug)
}

#[cfg(test)]
mod tests;
