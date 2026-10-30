//! A pool host: boots, pauses and deletes VM leases with the host's own crabbox, and carries
//! dials to a lease's sshd. The provider is the agent's own; a request names only the slug.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use moor_client::link::lease::{slug_id, Answer, Lease, Verb};
use moor_client::link::ops::Reply;
use tokio::process::Command;

pub use reach::{dial, splice, Sshd};

mod reach;

/// How long a VM may take to boot.
const BOOT: Duration = Duration::from_mins(15);
/// How long any other crabbox call may take.
const CALL: Duration = Duration::from_mins(2);
/// The exit code of `crabbox stop` for a lease that is already gone.
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
    /// The crabbox program.
    pub crabbox: PathBuf,
}

/// Does `verb` to the lease `slug`, or lists the leases, under `home`.
pub async fn lease(home: &Path, pool: &Pool, verb: Verb, slug: Option<&str>) -> Reply {
    match answer(home, pool, verb, slug).await {
        Ok(answer) => Reply::Leased(answer),
        Err(reason) => Reply::Refused { reason },
    }
}

/// What `verb` came to.
async fn answer(
    home: &Path,
    pool: &Pool,
    verb: Verb,
    slug: Option<&str>,
) -> Result<Answer, String> {
    match (verb, slug.map(checked).transpose()?) {
        (Verb::List, None) => Ok(Answer::Leases {
            leases: list(home, pool).await?,
        }),
        (Verb::List, Some(_)) | (_, None) => {
            Err("list names no slug; every other verb names one".to_owned())
        }
        (verb, Some(id)) => {
            act(
                &Crabbox {
                    home,
                    pool,
                    id: Some(id),
                },
                verb,
                id,
            )
            .await
        }
    }
}

/// Does `verb` to lease `id`.
async fn act(lease: &Crabbox<'_>, verb: Verb, id: i64) -> Result<Answer, String> {
    let ran = match verb {
        Verb::Warmup => {
            room(lease.home, lease.pool, id).await?;
            lease.run("warmup", "--slug", BOOT, &[0]).await
        }
        Verb::Info => return reach::info(lease).await,
        Verb::Pause => lease.run("pause", "--id", CALL, &[0]).await,
        Verb::Resume => lease.run("resume", "--id", CALL, &[0]).await,
        Verb::Stop => {
            let gone = lease.run("stop", "--id", CALL, &[0, GONE]).await;
            let _ = tokio::fs::remove_dir_all(lease_dir(lease.home, id)).await;
            gone
        }
        Verb::List => return Err("list names no slug".to_owned()),
    };
    match ran {
        Ok(_) => Ok(Answer::Done),
        Err(e) if verb == Verb::Pause && e.contains("does not support pause") => {
            Ok(Answer::Unsupported)
        }
        Err(e) => Err(e),
    }
}

/// The Box id of `slug`, when it is `moor-<id>`.
fn checked(slug: &str) -> Result<i64, String> {
    slug_id(slug).ok_or_else(|| format!("{slug:?} is not a moor-<id> slug"))
}

/// Ok unless booting `id` would pass the pool's most leases.
async fn room(home: &Path, pool: &Pool, id: i64) -> Result<(), String> {
    let Some(max) = pool.max else {
        return Ok(());
    };
    let slug = format!("moor-{id}");
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

/// The `moor-<id>` leases `crabbox list` shows. A lease names its slug in `labels.slug`, `slug`
/// or `name`, and its state in `state` or `status`, by provider.
async fn list(home: &Path, pool: &Pool) -> Result<Vec<Lease>, String> {
    let printed = Crabbox {
        home,
        pool,
        id: None,
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
                .filter(|s| slug_id(s).is_some())?;
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

/// Box `id`'s lease dir, which crabbox runs from.
fn lease_dir(home: &Path, id: i64) -> PathBuf {
    home.join(format!("moor-state/{id}/lease"))
}

/// crabbox on this host, for one lease or, with no id, for the pool.
struct Crabbox<'a> {
    /// The account's home.
    home: &'a Path,
    /// The pool.
    pool: &'a Pool,
    /// The lease's Box id.
    id: Option<i64>,
}

impl Crabbox<'_> {
    /// Runs `crabbox VERB --provider P FLAG moor-<id>`, or `FLAG` alone with no id, within
    /// `within`; its stdout when it exits with a code in `ok`.
    async fn run(
        &self,
        verb: &str,
        flag: &str,
        within: Duration,
        ok: &[i32],
    ) -> Result<String, String> {
        let base = self.home.join("moor-state");
        let ceiling = self
            .id
            .map_or_else(|| base.clone(), |id| base.join(id.to_string()));
        let dir = self
            .id
            .map_or_else(|| ceiling.clone(), |_| ceiling.join("lease"));
        std::fs::create_dir_all(&dir).map_err(|e| format!("cannot make {}: {e}", dir.display()))?;
        let mut cmd = Command::new(&self.pool.crabbox);
        cmd.args([verb, "--provider", &self.pool.provider, flag]);
        if let Some(id) = self.id {
            cmd.arg(format!("moor-{id}"));
        }
        cmd.current_dir(&dir)
            .env("HOME", self.home)
            .env("GIT_CEILING_DIRECTORIES", ceiling)
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
