//! A Box on a user's own host, `provider@host`: the host's agent leases its VM over the link,
//! and moor reaches the VM with ssh words it builds itself from the typed answers. No word or
//! command the host sends is ever run here.

use std::fs::{self, DirBuilder};
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use moor_client::link::lease::{Answer, Boot, CheckpointAct, Verb};
use moor_core::box_key::{parse_pool_slug, BoxKey, WORK_ROOT};
use moor_core::name::{BoxHandle, BoxPath, ProviderId};
use moor_core::seam::driver::{AcquireReq, Acquired, DriverCaps, DriverError, Released, SleepKind};

use super::{handle_of, pin, split_handle, Crabbox};

pub(super) mod checkpoints;
mod power;
pub(super) use power::{sleep, wake};
mod list;
pub(in crate::adapters::crabbox) use list::{list, list_handles};

/// How the controller reaches pool hosts: by enrolled machine id, through the runner's links
/// broker.
pub(crate) trait PoolHosts: Send + Sync {
    /// Asks machine `machine_id` to do `verb` to `slug`; an authorize carries public `key`, a
    /// warmup the `boot` it boots with.
    fn lease(
        &self,
        machine_id: i64,
        verb: Verb,
        slug: Option<&str>,
        key: Option<&str>,
        boot: &Boot,
    ) -> Result<Answer, String>;
    /// Asks machine `machine_id` to act on its checkpoints.
    fn checkpoint(&self, machine_id: i64, act: CheckpointAct) -> Result<Answer, String> {
        let _ = (machine_id, act);
        Err("this seam reaches no checkpoints".to_owned())
    }
    /// The ssh `ProxyCommand` that dials `slug`'s sshd on machine `machine_id`.
    fn dial_command(&self, machine_id: i64, slug: &str) -> Result<String, String>;
}

/// Env var holding the seconds the ssh call running the dial `ProxyCommand` waits; ssh hands
/// its env down to the command.
pub(crate) const DIAL_WITHIN: &str = "MOOR_DIAL_WITHIN";

/// A pool Box: its host's machine and its lease.
struct Placed<'a> {
    /// The hosts seam.
    hosts: &'a dyn PoolHosts,
    /// The host's enrolled machine id.
    machine: i64,
    /// `moor-<controller>-<id>`.
    slug: &'a str,
}

impl Placed<'_> {
    /// Does `verb` to the lease.
    fn ask(&self, verb: Verb, key: Option<&str>) -> Result<Answer, DriverError> {
        self.hosts
            .lease(self.machine, verb, Some(self.slug), key, &Boot::default())
            .map_err(failed)
    }

    /// Boots the lease's VM with `boot`, by `verb`.
    fn boot(&self, verb: Verb, boot: &Boot) -> Result<(), DriverError> {
        match self
            .hosts
            .lease(self.machine, verb, Some(self.slug), None, boot)
        {
            Ok(Answer::Done) => Ok(()),
            Ok(other) => Err(unexpected(verb, &other)),
            Err(e) => Err(failed(e)),
        }
    }

    /// Does `verb`, which must answer done.
    fn done(&self, verb: Verb, key: Option<&str>) -> Result<(), DriverError> {
        match self.ask(verb, key)? {
            Answer::Done => Ok(()),
            other => Err(unexpected(verb, &other)),
        }
    }
}

/// The base provider and host of `provider` when it names a host.
fn pool_provider(provider: &str) -> Option<(String, String)> {
    let id = ProviderId::parse(provider).ok()?;
    Some((id.base().to_owned(), id.host()?.to_owned()))
}

/// `<provider>/<machine id>/moor-<controller>-<box id>`: a pool Box's handle carries its host's machine.
fn pool_handle(provider: &str, machine: i64, slug: &str) -> BoxHandle {
    handle_of(provider, &format!("{machine}/{slug}"))
}

/// The handle `req` gets: its host's machine is known before the Box exists.
pub(super) fn planned(req: &AcquireReq) -> Result<BoxHandle, DriverError> {
    let seat = req.pool_machine.as_ref().ok_or_else(|| {
        failed(format!(
            "provider {} names a host, but the Box has no machine there",
            req.provider.as_str()
        ))
    })?;
    let slug = BoxKey::new(req.box_id).pool_slug(&seat.controller);
    Ok(pool_handle(req.provider.as_str(), seat.machine, &slug))
}

/// The machine id and lease slug of a pool handle's name part, parsed strictly.
fn parse_name(name: &str) -> Option<(i64, &str)> {
    let (machine, slug) = name.split_once('/')?;
    let digits = !machine.is_empty() && machine.bytes().all(|b| b.is_ascii_digit());
    let machine = machine.parse().ok().filter(|_| digits)?;
    parse_pool_slug(slug).is_some().then_some((machine, slug))
}

/// Whether `handle` names a pool Box.
pub(super) fn is_pool(handle: &BoxHandle) -> bool {
    split_handle(handle).0.and_then(pool_provider).is_some()
}

/// Whether `provider` names a host.
pub(super) fn is_pool_provider(provider: &ProviderId) -> bool {
    pool_provider(provider.as_str()).is_some()
}

/// A pool VM pauses and resumes through its host, and its reverse tunnel is ssh over moor's own
/// words; no cache volume reaches it yet, and its agent's offer says whether checkpoints do.
pub(super) const CAPS: DriverCaps = DriverCaps {
    sleep: SleepKind::Suspend,
    wake: true,
    reverse_tunnel: true,
    has_workdir: true,
    cache_volumes: false,
    cache_save_on_pause: false,
    checkpoints: true,
};

impl Crabbox {
    /// Reaches pool hosts through `hosts`; none refuses every pool Box.
    pub(crate) fn with_pool_hosts(mut self, hosts: Option<std::sync::Arc<dyn PoolHosts>>) -> Self {
        self.pool_hosts = hosts;
        self
    }

    /// The pool Box behind `handle`.
    fn placed<'a>(&'a self, handle: &'a BoxHandle) -> Result<Placed<'a>, DriverError> {
        let (provider, name) = split_handle(handle);
        let (machine, slug) = provider
            .and_then(pool_provider)
            .and_then(|_| parse_name(name))
            .ok_or_else(|| {
                failed(format!(
                    "{} is not a pool Box: its lease slug is not moor-<controller>-<id>",
                    handle.as_str()
                ))
            })?;
        let hosts = self
            .pool_hosts
            .as_deref()
            .ok_or_else(|| failed("this controller reaches no pool hosts"))?;
        Ok(Placed {
            hosts,
            machine,
            slug,
        })
    }

    /// `<state>/pool`, made 0700: the controller key and each Box's known hosts.
    fn pool_dir(&self) -> Result<PathBuf, DriverError> {
        let dir = self
            .leases
            .as_deref()
            .and_then(Path::parent)
            .ok_or_else(|| failed("moor has no state dir for pool keys"))?
            .join("pool");
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&dir)
            .map_err(|e| failed(format!("cannot make {}: {e}", dir.display())))?;
        Ok(dir)
    }

    /// The controller's key pair, made once; its private key path.
    fn controller_key(&self) -> Result<PathBuf, DriverError> {
        let key = self.pool_dir()?.join("id_ed25519");
        if !key.exists() {
            let made = Command::new("ssh-keygen")
                .args([
                    "-q",
                    "-t",
                    "ed25519",
                    "-N",
                    "",
                    "-C",
                    "moor-controller",
                    "-f",
                ])
                .arg(&key)
                .stdin(Stdio::null())
                .status();
            if !made.is_ok_and(|s| s.success()) {
                return Err(failed("cannot make the controller's ssh key"));
            }
        }
        Ok(key)
    }
}

/// Leases a VM for `req` on its host and lets the controller's key in; a failure after the
/// warmup stops the lease.
pub(super) fn acquire(cb: &Crabbox, req: &AcquireReq) -> Result<Acquired, DriverError> {
    boot_and_reach(cb, req, Verb::Warmup)
}

/// As [`acquire`], taking over the warmup or fork a cut-off caller left going on the host.
pub(super) fn adopt(cb: &Crabbox, req: &AcquireReq) -> Result<Acquired, DriverError> {
    boot_and_reach(cb, req, Verb::Adopt)
}

/// Whether `handle` names a lease on a user's pool host.
pub(super) fn is_pool_handle(handle: &BoxHandle) -> bool {
    split_handle(handle)
        .0
        .is_some_and(|p| is_pool_provider(&ProviderId::new(p)))
}

/// Has the host boot the VM with `verb`, then lets the controller's key in.
fn boot_and_reach(cb: &Crabbox, req: &AcquireReq, verb: Verb) -> Result<Acquired, DriverError> {
    let handle = planned(req)?;
    let placed = cb.placed(&handle)?;
    cb.forget_targets(&handle);
    // A Box with no size leaves the class to the host's provider, as an older agent knows no
    // class to take.
    let class = req.class.as_deref().map(super::size_of).transpose()?;
    placed.boot(
        verb,
        &Boot {
            cache: super::cache::lease(&req.cache),
            class: class.map(str::to_owned),
        },
    )?;
    let reached = cb.controller_key().and_then(|key| {
        let public = fs::read_to_string(key.with_extension("pub"))
            .map_err(|e| failed(format!("cannot read the controller's public key: {e}")))?;
        placed.done(Verb::Authorize, Some(public.trim()))?;
        let (user, _) = target(cb, &handle)?;
        Ok(user)
    });
    let workdir = |user: &str| {
        let root = format!("{}/{WORK_ROOT}", home_of(user));
        match req.cache.work_tree() {
            Some(tree) => format!("{root}/{tree}"),
            None => root,
        }
    };
    match reached {
        Ok(user) => Ok(Acquired {
            workdir: BoxPath::restore(&workdir(&user)).map_err(|e| failed(e.to_string()))?,
            handle,
        }),
        Err(e) => {
            let _ = release(cb, &handle);
            Err(e)
        }
    }
}

/// The VM's login home for `user`.
fn home_of(user: &str) -> String {
    if user == "root" {
        "/root".to_owned()
    } else {
        format!("/home/{user}")
    }
}

/// The VM's login user and the ssh words that reach it, from its host's typed sign-in.
pub(super) fn target(
    cb: &Crabbox,
    handle: &BoxHandle,
) -> Result<(String, Vec<String>), DriverError> {
    let placed = cb.placed(handle)?;
    let (user, host_keys) = match placed.ask(Verb::Info, None)? {
        Answer::Info { user, host_keys } => (user, host_keys),
        other => return Err(unexpected(Verb::Info, &other)),
    };
    if !valid_user(&user) {
        return Err(failed(format!(
            "the host named an unusable VM user {user:?}"
        )));
    }
    let keys: Vec<String> = host_keys
        .iter()
        .filter_map(|k| ssh_key::PublicKey::from_openssh(k).ok()?.to_openssh().ok())
        .collect();
    if keys.is_empty() {
        return Err(failed("the host sent no readable VM host key"));
    }
    let dir = cb.pool_dir()?;
    let known = dir.join(format!("{}.known_hosts", placed.slug));
    let lines: String = keys.iter().fold(String::new(), |mut all, k| {
        all.push_str(placed.slug);
        all.push(' ');
        all.push_str(k);
        all.push('\n');
        all
    });
    fs::write(&known, lines).map_err(|e| failed(format!("cannot write known hosts: {e}")))?;
    let words = vec![
        "-i".to_owned(),
        cb.controller_key()?.to_string_lossy().into_owned(),
        "-o".to_owned(),
        "IdentitiesOnly=yes".to_owned(),
        "-o".to_owned(),
        "StrictHostKeyChecking=yes".to_owned(),
        "-o".to_owned(),
        format!("UserKnownHostsFile={}", known.display()),
        "-o".to_owned(),
        format!("HostKeyAlias={}", placed.slug),
        "-o".to_owned(),
        format!(
            "ProxyCommand={}",
            placed
                .hosts
                .dial_command(placed.machine, placed.slug)
                .map_err(failed)?
        ),
        format!("{user}@{}", placed.slug),
    ];
    pin::check(cb, &words)?;
    if let Ok(mut map) = cb.targets.lock() {
        map.insert(handle.as_str().to_owned(), words.clone());
    }
    Ok((user, words))
}

/// `words`, cached for `handle`, with the `ProxyCommand` of the runner that holds its host's link
/// now, so the first call after a relink already dials the new runner; one store read.
pub(super) fn redialed(
    cb: &Crabbox,
    handle: &BoxHandle,
    mut words: Vec<String>,
) -> Result<Vec<String>, DriverError> {
    let placed = cb.placed(handle)?;
    let dial = placed.hosts.dial_command(placed.machine, placed.slug);
    let now = format!("ProxyCommand={}", dial.map_err(failed)?);
    let Some(word) = words.iter_mut().find(|w| w.starts_with("ProxyCommand=")) else {
        return Ok(words);
    };
    if *word != now {
        *word = now;
        if let Ok(mut map) = cb.targets.lock() {
            map.insert(handle.as_str().to_owned(), words.clone());
        }
    }
    Ok(words)
}

/// A login name: `[a-z_][a-z0-9_-]*`.
fn valid_user(user: &str) -> bool {
    let mut bytes = user.bytes();
    bytes
        .next()
        .is_some_and(|b| b.is_ascii_lowercase() || b == b'_')
        && bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_-".contains(&b))
}

/// Stops the lease; one already gone counts as deleted.
pub(super) fn release(cb: &Crabbox, handle: &BoxHandle) -> Result<Released, DriverError> {
    let placed = cb.placed(handle)?;
    let stopped = placed.done(Verb::Stop, None);
    if let Ok(mut map) = cb.targets.lock() {
        map.remove(handle.as_str());
    }
    stopped?;
    if let Ok(dir) = cb.pool_dir() {
        let _ = fs::remove_file(dir.join(format!("{}.known_hosts", placed.slug)));
    }
    Ok(Released::Deleted)
}

/// A failure saying `why`.
fn failed(why: impl Into<String>) -> DriverError {
    DriverError::Failed {
        summary: why.into(),
    }
}

/// The failure of an answer that does not fit `verb`.
fn unexpected(verb: Verb, answer: &Answer) -> DriverError {
    failed(format!("the host answered {verb:?} with {answer:?}"))
}

#[cfg(test)]
mod tests;
