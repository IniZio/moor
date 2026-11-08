//! A pool host's checkpoints: saves, forks and deletes them with the host's own crabbox, and
//! reads the disk they fill. A request carries data only, each word checked before crabbox sees it.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use moor_client::link::lease::{Answer, Cache, CheckpointAct};
use moor_client::link::ops::Reply;
use moor_core::domain::event::{ProviderStep, MAX_STEPS};
use serde::Deserialize;
use tokio::process::Command;
use tokio::time::Instant;

use super::memory::disk_mib;
use super::{
    box_dir, cache, checked, checked_class, flight, room, stop_now, Crabbox, Pool, POOL_DIR,
};
use crate::settings;

/// How long a save or a fork may take, since it copies a VM's memory and disk.
const CHECKPOINT: Duration = Duration::from_mins(10);
/// How long `crabbox doctor` may take before the host goes without checkpoints; every dial waits
/// on it.
const DOCTOR: Duration = Duration::from_secs(15);
/// The feature that saves checkpoints.
const SAVES: &str = "workspace-checkpoint";
/// The feature that forks leases from them.
const FORKS: &str = "workspace-fork";
/// The longest reason an offer carries, from the end of crabbox's words.
const REASON: usize = 300;

/// Why the pool does not serve checkpoints, for `features` of its provider; none when it does.
pub(super) async fn off(home: &Path, pool: &Pool, features: &[String]) -> Option<String> {
    let config = settings::config_path(home);
    if let Some(key) = settings::opted_out(&config, &pool.provider) {
        return Some(format!("{key} is false in {}", config.display()));
    }
    for feature in [SAVES, FORKS] {
        if !features.iter().any(|f| f == feature) {
            return Some(format!(
                "crabbox provider {} lacks {feature}",
                pool.provider
            ));
        }
    }
    let mut cmd = Command::new(&pool.crabbox);
    cmd.args(["doctor", "--provider", &pool.provider])
        .env("HOME", home)
        .stdin(Stdio::null())
        .kill_on_drop(true);
    let out = match tokio::time::timeout(DOCTOR, cmd.output()).await {
        Err(_) => return Some(format!("crabbox doctor ran past {}s", DOCTOR.as_secs())),
        Ok(Err(e)) => return Some(format!("cannot run crabbox doctor: {e}")),
        Ok(Ok(out)) => out,
    };
    if out.status.success() {
        return None;
    }
    let (said, err) = (
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    let reasons = doctor_failures(&said, &err);
    let words = if reasons.is_empty() {
        format!("{said}\n{err}")
    } else {
        reasons.join("; ")
    };
    let words = words.trim();
    let cut = (words.len().saturating_sub(REASON)..=words.len())
        .find(|at| words.is_char_boundary(*at))
        .unwrap_or_default();
    let tail = words.get(cut..).unwrap_or_default();
    Some(format!("crabbox doctor failed: {tail}"))
}

/// What failed in doctor's output, each once: the adapter's own `FAIL` lines, which the provider
/// check carries among its passing ones, and crabbox's `failed` checks other than that carrier.
pub(crate) fn doctor_failures<'a>(said: &'a str, err: &'a str) -> Vec<&'a str> {
    let all = || said.lines().chain(err.lines());
    let adapter = all().filter(|l| l.starts_with("FAIL "));
    let carrier = |l: &&str| l.starts_with("failed") && l.contains("external provider command");
    let crabbox = all().filter(|l| l.starts_with("failed") && !carrier(l));
    let mut found: Vec<&str> = Vec::new();
    for line in adapter.chain(crabbox) {
        if !found.contains(&line) {
            found.push(line);
        }
    }
    found
}

/// Serves `act` under `home`.
pub async fn checkpoint_op(home: &Path, pool: &Pool, act: CheckpointAct) -> Reply {
    match answer(home, pool, act).await {
        Ok(answer) => Reply::Leased(answer),
        Err(reason) => Reply::Refused { reason },
    }
}

/// What `act` came to.
async fn answer(home: &Path, pool: &Pool, act: CheckpointAct) -> Result<Answer, String> {
    match act {
        CheckpointAct::CheckpointCreate { slug, name } => create(home, pool, &slug, &name).await,
        CheckpointAct::CheckpointFork {
            id,
            slug,
            cache,
            class,
        } => fork(home, pool, &id, &slug, cache.as_ref(), class.as_deref()).await,
        CheckpointAct::CheckpointDelete { id } => delete(home, pool, &id).await,
        CheckpointAct::Disk {} => {
            let (free_mib, total_mib) =
                disk_mib(home).ok_or("the checkpoint filesystem cannot be read")?;
            Ok(Answer::Disk {
                free_mib,
                total_mib,
            })
        }
    }
}

/// `word` itself when it can only be read as a checkpoint's id or name: letters, digits, `.`, `_`
/// and `-`, starting with a letter or digit, at most 128 bytes.
fn checked_word(word: &str) -> Result<&str, String> {
    let token = word
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
    let leads = word
        .bytes()
        .next()
        .is_some_and(|b| b.is_ascii_alphanumeric());
    if token && leads && word.len() <= 128 {
        Ok(word)
    } else {
        Err(format!("{word:?} is not a checkpoint id or name"))
    }
}

/// Saves lease `slug` from its own dir, which only a warmup or a fork makes.
async fn create(home: &Path, pool: &Pool, slug: &str, name: &str) -> Result<Answer, String> {
    let (slug, name) = (checked(slug)?, checked_word(name)?);
    let mine = box_dir(home, slug);
    let dir = mine.join("lease");
    if !dir.is_dir() {
        return Err(format!("this host holds no lease {slug}"));
    }
    let crabbox = Crabbox {
        home,
        pool,
        slug: Some(slug),
    };
    let args = [
        "checkpoint",
        "create",
        "--provider",
        &pool.provider,
        "--id",
        slug,
        "--name",
        name,
        "--mode",
        "native",
        "--json",
    ];
    let printed = crabbox
        .spawn(&args, (&dir, &mine), CHECKPOINT, &[0], &[])
        .await?;
    made(&printed)
}

/// What `checkpoint create --json` prints; unknown fields are ignored.
#[derive(Deserialize)]
struct Made {
    /// The checkpoint's id.
    id: String,
    /// The provider's own record of it.
    #[serde(default)]
    native: Option<Native>,
}

/// The provider part of a checkpoint record.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Native {
    /// Size on the host.
    #[serde(default)]
    bytes: Option<u64>,
    /// What only this checkpoint holds, without the checkpoints it is a delta over.
    #[serde(default)]
    unique_bytes: Option<u64>,
    /// The checkpoint it is a delta over.
    #[serde(default)]
    parent_checkpoint_id: Option<String>,
}

/// The answer to a create that printed `printed`; an id or parent that could not be sent back
/// to this agent later is refused now.
fn made(printed: &str) -> Result<Answer, String> {
    let made: Made = serde_json::from_str(printed.trim())
        .map_err(|e| format!("crabbox checkpoint create printed no record: {e}"))?;
    checked_word(&made.id)?;
    let native = made.native;
    let bytes = native.as_ref().and_then(|n| n.unique_bytes.or(n.bytes));
    let parent = native.and_then(|n| n.parent_checkpoint_id);
    if let Some(parent) = &parent {
        checked_word(parent)?;
    }
    Ok(Answer::Checkpoint {
        id: made.id,
        bytes,
        parent,
    })
}

/// Boots lease `slug` from checkpoint `id` in its own new dir; one that fails leaves no lease.
async fn fork(
    home: &Path,
    pool: &Pool,
    id: &str,
    slug: &str,
    cache: Option<&Cache>,
    class: Option<&str>,
) -> Result<Answer, String> {
    let (id, slug) = (checked_word(id)?, checked(slug)?);
    let class = class.map(checked_class).transpose()?;
    let env = cache.map(cache::env).transpose()?.unwrap_or_default();
    let began = Instant::now();
    room(home, pool, slug).await?;
    let room_ms = began.elapsed().as_millis();
    let mine = box_dir(home, slug);
    let (at, owned_home, owned_pool) = (mine.clone(), home.to_path_buf(), pool.clone());
    let (id, slug, class) = (id.to_owned(), slug.to_owned(), class.map(str::to_owned));
    let told = std::sync::Arc::new(std::sync::Mutex::new(None));
    let tell = std::sync::Arc::clone(&told);
    flight::fly(&mine, async move {
        let dir = at.join("lease");
        let existed = dir.is_dir();
        let mut args = vec!["checkpoint", "fork", id.as_str(), "--slug", slug.as_str()];
        if let Some(class) = class.as_deref() {
            args.extend(["--class", class]);
        }
        args.push("--json");
        let crabbox = Crabbox {
            home: &owned_home,
            pool: &owned_pool,
            slug: Some(&slug),
        };
        let env: Vec<(&str, String)> = env;
        let began = Instant::now();
        let forked = crabbox
            .spawn_both(&args, (&dir, &at), CHECKPOINT, &[0], &env)
            .await
            .and_then(|(printed, said)| same_lease(&printed, &slug).map(|()| said));
        let crabbox_ms = began.elapsed().as_millis();
        if forked.is_err() && !existed {
            let _ = stop_now(&crabbox, &slug).await;
        }
        forked.map(|said| {
            *tell
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some((crabbox_ms, said));
        })
    })
    .await?;
    // A fork another caller started and this one joined reports no timing.
    let (crabbox_ms, said) = told
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take()
        .unwrap_or_default();
    Ok(Answer::Forked {
        timing: timing_lines(room_ms, crabbox_ms, &said),
    })
}

/// This agent's own steps, which split the time around crabbox, then the steps crabbox's stderr
/// reports. Bounded here because a reply crosses the link whole.
fn timing_lines(room_ms: u128, crabbox_ms: u128, said: &str) -> Vec<String> {
    let own = [
        format!("timing agent-room ms={room_ms}"),
        format!("timing agent-crabbox ms={crabbox_ms}"),
    ];
    let reported = said
        .lines()
        .filter(|line| ProviderStep::parse(line).is_some())
        .map(str::to_owned);
    own.into_iter().chain(reported).take(MAX_STEPS).collect()
}

/// Ok unless `printed` names a lease other than `slug`.
fn same_lease(printed: &str, slug: &str) -> Result<(), String> {
    /// What `checkpoint fork --json` prints of the new lease.
    #[derive(Deserialize)]
    struct Forked {
        /// The lease's slug, when printed.
        #[serde(default)]
        slug: Option<String>,
    }
    let forked: Forked = serde_json::from_str(printed.trim())
        .map_err(|e| format!("crabbox checkpoint fork printed no record: {e}"))?;
    match forked.slug {
        Some(got) if got != slug => Err(format!(
            "crabbox forked the checkpoint as {got:?}, not {slug:?}"
        )),
        _ => Ok(()),
    }
}

/// Deletes checkpoint `id`; crabbox saying it does not know the id counts as deleted.
async fn delete(home: &Path, pool: &Pool, id: &str) -> Result<Answer, String> {
    let id = checked_word(id)?;
    let base = home.join(POOL_DIR);
    let crabbox = Crabbox {
        home,
        pool,
        slug: None,
    };
    let args = ["checkpoint", "delete", id];
    match crabbox
        .spawn(&args, (&base, &base), CHECKPOINT, &[0], &[])
        .await
    {
        Err(e) if !e.contains("not found") => Err(e),
        _ => Ok(Answer::Done),
    }
}
