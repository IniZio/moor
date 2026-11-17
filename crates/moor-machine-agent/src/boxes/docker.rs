//! Boxes made of Docker containers, run with the stock `docker` command line.
//!
//! A Box is two containers, the agent place and the work place, on one network. The agent place
//! owns the network namespace and the work place joins it, so the agent place starts first and
//! the work place restarts after it. Both places mount the same work tree and run volume, which
//! `images/README.md` describes. Docker restarts nothing (`--restart no`): this agent is the only
//! actor that restarts a place.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::Arc;

use moor_client::link::ops;
use moor_core::seam::driver::Place;
use tokio::process::Command;

use super::provider::{BoxProvider, Env, Notes, Observed, PlaceState, ProvideError, Spec};
use super::Slug;
use args::{agent_run, kvm_gid, work_memory_mib, work_run, PLACE_LABEL, SLUG_LABEL, WORKDIR};
use cli::{Cli, State};
use names::Names;

mod args;
mod cli;
#[cfg(test)]
mod live;
mod names;
mod net;
#[cfg(test)]
mod tests;

/// The agent place waits this many half seconds for its init to make the key and the tools.
const AGENT_READY_TRIES: u32 = 60;
/// An open waits this many half seconds for the work place to answer over ssh.
const WORK_READY_TRIES: u32 = 180;
/// The work place gets this long to stop its Docker and sshd before Docker kills it.
const STOP_SECONDS: &str = "10";
/// The network Boxes join unless told otherwise.
const NETWORK: &str = "moor";

/// Which images make a Box; a part left out is the release's own image.
#[derive(Debug, Clone, Default)]
pub struct Images {
    /// The agent place's image.
    pub agent: Option<String>,
    /// The work place's image.
    pub work: Option<String>,
}

/// What the provider is set up with.
#[derive(Debug, Clone)]
pub(super) struct Settings {
    /// The agent place's image.
    pub(super) agent_image: String,
    /// The work place's image.
    pub(super) work_image: String,
    /// The network both places join, made on first use.
    pub(super) network: String,
}

impl Settings {
    /// The images of `images`, and the release's for any it leaves out.
    fn new(images: Images, network: &str) -> Self {
        let release = |name: &str| format!("ghcr.io/inizio/{name}:{}", crate::VERSION);
        Self {
            agent_image: images.agent.unwrap_or_else(|| release("moor-agent")),
            work_image: images.work.unwrap_or_else(|| release("moor-work")),
            network: network.to_owned(),
        }
    }
}

/// The Docker provider. Cloning shares it, which an open needs to outlive its caller.
#[derive(Clone)]
pub(crate) struct Docker {
    /// What it is set up with.
    settings: Arc<Settings>,
    /// One at a time may create the network.
    network: Arc<tokio::sync::Mutex<()>>,
}

impl Docker {
    /// A provider making Boxes from `images` on the network `network`.
    pub(crate) fn new(images: Images, network: Option<&str>) -> Self {
        Self {
            settings: Arc::new(Settings::new(images, network.unwrap_or(NETWORK))),
            network: Arc::default(),
        }
    }

    /// Pulls the images that are not on this host.
    async fn ensure_images(&self, note: &Notes) -> Result<(), ProvideError> {
        for image in [&self.settings.agent_image, &self.settings.work_image] {
            if !Cli::passes(["image", "inspect", image]).await {
                note.say(&format!("pulling {image}"));
                Cli::ok(["pull", "-q", image]).await?;
            }
        }
        Ok(())
    }

    /// Makes what is missing of `slug` and starts what is stopped, and waits until the work place
    /// answers.
    async fn open_now(&self, slug: &Slug, spec: &Spec, note: &Notes) -> Result<(), ProvideError> {
        let names = Names::of(slug);
        self.ensure_images(note).await?;
        {
            let _one = self.network.lock().await;
            net::ensure(&self.settings.network, note).await?;
        }
        for volume in names.volumes() {
            let label = format!("{SLUG_LABEL}={}", slug.as_str());
            Cli::ok(["volume", "create", "--label", &label, volume]).await?;
        }
        note.say("starting the Box");
        self.start_places(slug, Some(spec)).await?;
        self.await_work(&names).await
    }

    /// Starts the agent place, then the work place. A work place that runs while the agent place
    /// was just started holds the old network namespace, so it restarts. With `create` the places
    /// that are missing are made; without it a missing place is [`ProvideError::Missing`].
    async fn start_places(&self, slug: &Slug, create: Option<&Spec>) -> Result<(), ProvideError> {
        let names = Names::of(slug);
        let fresh = self.start_agent(slug, &names, create.is_some()).await?;
        // A running agent place may be one an open cut off by a restart made, not ready yet.
        self.await_agent(&names).await?;
        match (Cli::state(&names.work).await?, create) {
            (Some(State::Running), _) if fresh => {
                Cli::ok(["restart", "-t", STOP_SECONDS, &names.work]).await?;
            }
            (Some(State::Running), _) => {}
            (Some(State::Stopped), _) => {
                Cli::ok(["start", &names.work]).await?;
            }
            (None, Some(spec)) => {
                let mem = work_memory_mib(spec.class.as_deref());
                Cli::ok(work_run(&self.settings, slug, &names, mem, kvm_gid())).await?;
            }
            (None, None) => return Err(ProvideError::Missing),
        }
        Ok(())
    }

    /// Starts the agent place, making it when `create`; whether it is a new run of it, which gives
    /// it a network namespace of its own.
    async fn start_agent(
        &self,
        slug: &Slug,
        names: &Names,
        create: bool,
    ) -> Result<bool, ProvideError> {
        match Cli::state(&names.agent).await? {
            Some(State::Running) => Ok(false),
            Some(State::Stopped) => {
                Cli::ok(["start", &names.agent]).await?;
                Ok(true)
            }
            None if create => {
                Cli::ok(agent_run(&self.settings, slug, names)).await?;
                Ok(true)
            }
            None => Err(ProvideError::Missing),
        }
    }

    /// Waits for the agent place's init to make the key and the tools the work place starts from.
    async fn await_agent(&self, names: &Names) -> Result<(), ProvideError> {
        for _ in 0..AGENT_READY_TRIES {
            let ready = ["exec", &names.agent, "test", "-e", "/run/moor/agent.ready"];
            if Cli::passes(ready).await {
                return Ok(());
            }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
        Err(ProvideError::Failed(
            "the agent place did not become ready".to_owned(),
        ))
    }

    /// Waits until a command in the work place runs, which is when its sshd answers.
    async fn await_work(&self, names: &Names) -> Result<(), ProvideError> {
        for _ in 0..WORK_READY_TRIES {
            if Cli::passes(["exec", &names.agent, "box-shell", "-c", "true"]).await {
                return Ok(());
            }
            if Cli::state(&names.work).await? != Some(State::Running) {
                let logs = Cli::output(["logs", "--tail", "20", &names.work]).await?;
                let tail = String::from_utf8_lossy(&logs.stderr);
                return Err(ProvideError::Failed(format!(
                    "the work place stopped while starting: {}",
                    tail.trim()
                )));
            }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
        Err(ProvideError::Failed(
            "the work place did not answer over ssh".to_owned(),
        ))
    }

    /// Writes `content` to `path` in the agent place, through a rename so a reader never sees half.
    async fn write_run_file(
        &self,
        names: &Names,
        path: &str,
        content: &str,
    ) -> Result<(), ProvideError> {
        let script = format!("cat > {path}.new && mv -f {path}.new {path}");
        Cli::feed(["exec", "-i", &names.agent, "sh", "-c", &script], content).await
    }

    /// The OOM kills the work place's memory limit has made, from the run volume.
    async fn oom_kills(&self, names: &Names) -> u32 {
        Cli::ok(["exec", &names.agent, "cat", "/run/moor/oom"])
            .await
            .ok()
            .and_then(|text| text.parse().ok())
            .unwrap_or(0)
    }
}

/// The `box.env` file for `env`: one `export` per variable.
fn env_file(env: &Env) -> String {
    env.vars
        .iter()
        .fold(String::new(), |mut file, (name, value)| {
            let _ = writeln!(file, "export {name}={}", ops::quote(value));
            file
        })
}

/// The places of each Box that `docker ps` lists as `slug place state` lines: how the agent place
/// and the work place stand. A line that is not a Box's is left out.
fn parse_ps(listed: &str) -> BTreeMap<Slug, (PlaceState, PlaceState)> {
    let mut boxes: BTreeMap<Slug, (PlaceState, PlaceState)> = BTreeMap::new();
    for line in listed.lines() {
        let mut words = line.split_whitespace();
        let (Some(slug), Some(place), Some(state)) = (
            words.next().and_then(Slug::parse),
            words.next(),
            words.next(),
        ) else {
            continue;
        };
        let up = if state == "running" {
            PlaceState::Up
        } else {
            PlaceState::Down
        };
        let places = boxes
            .entry(slug)
            .or_insert((PlaceState::Down, PlaceState::Down));
        match place {
            "agent" => places.0 = up,
            "work" => places.1 = up,
            _ => {}
        }
    }
    boxes
}

impl BoxProvider for Docker {
    fn provides(&self) -> &'static str {
        "docker"
    }

    async fn open(&self, slug: &Slug, spec: &Spec, note: &Notes) -> Result<String, ProvideError> {
        // The open runs as a spawn of its own, so a caller that goes away mid-pull does not leave
        // a Box half made and nobody finishing it.
        let (me, slug, spec, note) = (self.clone(), slug.clone(), spec.clone(), note.clone());
        tokio::spawn(async move { me.open_now(&slug, &spec, &note).await })
            .await
            .map_err(|e| ProvideError::Failed(format!("the open did not finish: {e}")))??;
        Ok(WORKDIR.to_owned())
    }

    async fn start(&self, slug: &Slug) -> Result<(), ProvideError> {
        self.start_places(slug, None).await?;
        self.await_work(&Names::of(slug)).await
    }

    async fn stop(&self, slug: &Slug) -> Result<(), ProvideError> {
        let names = Names::of(slug);
        if Cli::state(&names.agent).await?.is_none() {
            return Err(ProvideError::Missing);
        }
        for place in [&names.work, &names.agent] {
            if Cli::state(place).await? == Some(State::Running) {
                Cli::ok(["stop", "-t", STOP_SECONDS, place]).await?;
            }
        }
        Ok(())
    }

    async fn remove(&self, slug: &Slug) -> Result<(), ProvideError> {
        let names = Names::of(slug);
        for place in [&names.work, &names.agent] {
            if Cli::state(place).await?.is_some() {
                Cli::ok(["rm", "-f", place]).await?;
            }
        }
        for volume in names.volumes() {
            Cli::ok(["volume", "rm", "-f", volume]).await?;
        }
        Ok(())
    }

    /// Sealing, a `docker commit` of the work place, is not built yet; nothing starts a repair
    /// from a baseline until the mend loop does.
    #[allow(
        clippy::unused_async_trait_impl,
        reason = "a stub has nothing to await yet"
    )]
    async fn seal(&self, _slug: &Slug) -> Result<(), ProvideError> {
        Err(ProvideError::Failed(
            "sealing a Docker Box is not supported yet".to_owned(),
        ))
    }

    async fn apply_env(&self, slug: &Slug, env: &Env) -> Result<(), ProvideError> {
        let names = Names::of(slug);
        match Cli::state(&names.agent).await? {
            None => return Err(ProvideError::Missing),
            Some(State::Stopped) => {
                return Err(ProvideError::Failed(
                    "the Box is asleep; its environment is set while it runs".to_owned(),
                ))
            }
            Some(State::Running) => {}
        }
        self.write_run_file(&names, "/run/moor/box.env", &env_file(env))
            .await?;
        match &env.ca_pem {
            Some(pem) => {
                self.write_run_file(&names, "/run/moor/ca.pem", pem).await?;
            }
            None => {
                Cli::ok(["exec", &names.agent, "rm", "-f", "/run/moor/ca.pem"]).await?;
            }
        }
        if Cli::state(&names.work).await? == Some(State::Running) {
            Cli::ok(["exec", &names.work, "/run/moor/bin/moor-work-init", "--env"]).await?;
        }
        Ok(())
    }

    async fn observe(&self) -> Result<Vec<Observed>, ProvideError> {
        let format = format!(
            "{{{{.Label \"{SLUG_LABEL}\"}}}} {{{{.Label \"{PLACE_LABEL}\"}}}} {{{{.State}}}}"
        );
        let listed = Cli::ok([
            "ps",
            "-a",
            "--filter",
            &format!("label={SLUG_LABEL}"),
            "--format",
            &format,
        ])
        .await?;
        let mut boxes = Vec::new();
        for (slug, (agent, work)) in parse_ps(&listed) {
            let oom_kills = if agent == PlaceState::Up {
                self.oom_kills(&Names::of(&slug)).await
            } else {
                0
            };
            boxes.push(Observed {
                slug,
                agent,
                work,
                oom_kills,
                mem_mib: None,
            });
        }
        Ok(boxes)
    }

    fn carrier(&self, slug: &Slug, place: Place, tty: bool) -> Command {
        let names = Names::of(slug);
        let mut args = vec!["exec", "-i"];
        if tty {
            args.push("-t");
        }
        args.extend(["-w", WORKDIR, &names.agent]);
        // Both places run a login shell, which is what reads the Box's environment. The work
        // place is reached through box-shell, which says "down" with exit 75.
        args.extend(match place {
            Place::Agent => ["bash", "-lc"],
            Place::Work => ["box-shell", "-c"],
        });
        Cli::command(args)
    }
}
