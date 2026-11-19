//! Boxes this machine provides, served over link streams as the Box protocol.
//!
//! A [`BoxProvider`] makes the Box; everything above it is written once: parsing a request,
//! one flight at a time per Box, the held state, `exec` and `forward` over the provider's
//! carrier, and the health report. A Box has two places, the agent's and the work place's, and the
//! controller names one on each command without learning what it is made of.

use std::collections::BTreeMap;
use std::io;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use moor_client::link::boxes::{BoxAnswer, BoxOffer, BoxOp, BoxReport, ReportState, PROTOCOL};
use moor_client::link::ops;
use moor_core::box_key::{parse_pool_slug, ControllerId};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};

mod docker;
#[cfg(any(test, feature = "fake-boxes"))]
mod fake;
mod held;
mod host;
mod inside;
mod provider;
mod stream;
#[cfg(test)]
mod tests;

pub use docker::Images;
pub use host::Host;

#[cfg(any(test, feature = "fake-boxes"))]
use fake::FakeProvider;
use held::{Held, Store, Want};
use provider::{BoxProvider, Ended, Env, Notes, Observed, PlaceState, ProvideError, Spec};

/// A Box's name on its machine: `moor-<controller>-<id>[-h<inc>]`, so two controllers sharing a
/// machine never touch each other's Boxes. Only [`Slug::parse`] makes one, which keeps every
/// path built from it inside the Box's own names.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Slug(String);

impl Slug {
    /// The slug `text` spells; none for any other text.
    pub(crate) fn parse(text: &str) -> Option<Self> {
        parse_pool_slug(text).map(|_| Self(text.to_owned()))
    }

    /// The slug as text.
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    /// The controller the slug names.
    fn controller(&self) -> Option<ControllerId> {
        parse_pool_slug(&self.0).map(|(controller, ..)| controller)
    }
}

/// A stream the Box protocol runs on.
pub(crate) trait Wire: AsyncRead + AsyncWrite + Unpin + Send + 'static {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send + 'static> Wire for T {}

/// The Boxes one provider holds for this machine.
pub(crate) struct Boxes<P: BoxProvider> {
    /// How the Boxes are made.
    provider: P,
    /// What is kept per Box.
    store: Store,
    /// One lock per Box with a lifecycle verb going on, so two never run at once on the same Box.
    flights: Mutex<BTreeMap<Slug, Arc<tokio::sync::Mutex<()>>>>,
}

impl<P: BoxProvider> Boxes<P> {
    /// The Boxes `provider` makes, holding their state under `home`.
    pub(crate) fn new(provider: P, home: &Path) -> Self {
        Self {
            provider,
            store: Store::under(home),
            flights: Mutex::default(),
        }
    }

    /// What the machine offers.
    pub(crate) fn offer(&self) -> BoxOffer {
        BoxOffer {
            protocol: PROTOCOL,
            provides: self.provider.provides().to_owned(),
        }
    }

    /// Whether the provider keeps cache volumes across Boxes.
    pub(crate) fn keeps_caches(&self) -> bool {
        self.provider.keeps_caches()
    }

    /// Serves the one request on `stream`: reads its line, answers, and for an exec or a forward
    /// carries the stream. Anything that is not a Box request is refused.
    pub(crate) async fn serve<S: Wire>(self: &Arc<Self>, mut stream: S) {
        let op = match ops::read_line::<_, BoxOp>(&mut stream).await {
            Ok(op) => op,
            Err(e) => return answer(&mut stream, refused(format!("not a Box request: {e}"))).await,
        };
        match op {
            BoxOp::Exec { .. } => self.exec(&op, stream).await,
            BoxOp::Forward { .. } => self.forward(&op, stream).await,
            op => self.lifecycle(op, stream).await,
        }
    }

    /// Serves a verb that answers and ends: notes while it works, then its answer. The verb runs
    /// on a spawn of its own, so a controller that goes away mid-verb leaves it to finish and keep
    /// its held state instead of leaving a Box half made.
    async fn lifecycle<S: Wire>(self: &Arc<Self>, op: BoxOp, mut stream: S) {
        let (notes, mut said) = Notes::new();
        let me = Arc::clone(self);
        let mut work = tokio::spawn(async move { me.verb(op, &notes).await });
        let done = loop {
            tokio::select! {
                done = &mut work => {
                    break done.unwrap_or_else(|e| refused(format!("the request failed: {e}")));
                }
                Some(line) = said.recv() => answer(&mut stream, BoxAnswer::Note { line }).await,
            }
        };
        while let Ok(line) = said.try_recv() {
            answer(&mut stream, BoxAnswer::Note { line }).await;
        }
        answer(&mut stream, done).await;
        let _ = stream.shutdown().await;
    }

    /// What `op` comes to.
    async fn verb(&self, op: BoxOp, notes: &Notes) -> BoxAnswer {
        let done = match op {
            BoxOp::Open { slug, class, cache } => {
                self.open(&slug, Spec { class, cache }, notes).await
            }
            BoxOp::Wake { slug } => self.wake(&slug).await,
            BoxOp::Sleep { slug } => self.sleep(&slug).await,
            BoxOp::Close { slug } => self.close(&slug).await,
            BoxOp::Seal { slug } => self.seal(&slug).await,
            BoxOp::Env { slug, vars, ca_pem } => self.env(&slug, Env { vars, ca_pem }).await,
            BoxOp::List { controller } => Ok(self.list(Some(&controller)).await),
            BoxOp::Exec { .. } | BoxOp::Forward { .. } => Err("not a verb that ends".to_owned()),
        };
        done.unwrap_or_else(refused)
    }

    /// `slug` as a [`Slug`], and the lock that keeps another verb off it until dropped.
    async fn flight(&self, slug: &str) -> Result<(Slug, tokio::sync::OwnedMutexGuard<()>), String> {
        let slug = Slug::parse(slug).ok_or_else(|| format!("{slug:?} is not a Box slug"))?;
        let lock = Arc::clone(
            self.flights
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .entry(slug.clone())
                .or_default(),
        );
        Ok((slug, lock.lock_owned().await))
    }

    /// `slug`'s held state; none when the Box was never opened here or was closed.
    fn held(&self, slug: &Slug) -> Result<Option<Held>, String> {
        self.store
            .load(slug)
            .map_err(|e| format!("cannot read the Box's held state: {e}"))
    }

    /// Keeps `held` for `slug`.
    fn keep(&self, slug: &Slug, held: &Held) -> Result<(), String> {
        self.store
            .save(slug, held)
            .map_err(|e| format!("cannot keep the Box's held state: {e}"))
    }

    /// Makes the Box, or finds it, and starts it.
    async fn open(&self, slug: &str, spec: Spec, notes: &Notes) -> Result<BoxAnswer, String> {
        let (slug, _flight) = self.flight(slug).await?;
        let mut held = self.held(&slug)?.unwrap_or_else(|| Held::new(spec.clone()));
        let restarting = held.want == Want::Asleep;
        held.spec = spec;
        let workdir = self
            .provider
            .open(&slug, &held.spec, notes)
            .await
            .map_err(|e| e.to_string())?;
        if held.box_env != Env::default() {
            let env = self.provider.apply_env(&slug, &held.box_env);
            env.await.map_err(|e| e.to_string())?;
        }
        if held.generation == 0 || restarting {
            held.generation += 1;
        }
        held.want = Want::Awake;
        held.broken = None;
        self.keep(&slug, &held)?;
        Ok(BoxAnswer::Opened {
            workdir,
            generation: held.generation,
        })
    }

    /// Starts a sleeping Box.
    async fn wake(&self, slug: &str) -> Result<BoxAnswer, String> {
        let (slug, _flight) = self.flight(slug).await?;
        let Some(mut held) = self.held(&slug)? else {
            return Ok(BoxAnswer::Gone);
        };
        match self.provider.start(&slug).await {
            Err(ProvideError::Missing) => return Ok(BoxAnswer::Gone),
            other => other.map_err(|e| e.to_string())?,
        }
        if held.want == Want::Asleep {
            held.generation += 1;
        }
        held.want = Want::Awake;
        self.keep(&slug, &held)?;
        Ok(BoxAnswer::Woke {
            generation: held.generation,
        })
    }

    /// Stops every process in the Box.
    async fn sleep(&self, slug: &str) -> Result<BoxAnswer, String> {
        let (slug, _flight) = self.flight(slug).await?;
        let Some(mut held) = self.held(&slug)? else {
            return Ok(BoxAnswer::Gone);
        };
        match self.provider.stop(&slug).await {
            Err(ProvideError::Missing) => return Ok(BoxAnswer::Gone),
            other => other.map_err(|e| e.to_string())?,
        }
        held.want = Want::Asleep;
        self.keep(&slug, &held)?;
        Ok(BoxAnswer::Done)
    }

    /// Removes the Box; one already gone is fine.
    async fn close(&self, slug: &str) -> Result<BoxAnswer, String> {
        let (slug, _flight) = self.flight(slug).await?;
        match self.provider.remove(&slug).await {
            Err(ProvideError::Missing) | Ok(()) => {}
            Err(e) => return Err(e.to_string()),
        }
        self.store
            .forget(&slug)
            .map_err(|e| format!("cannot forget the Box's held state: {e}"))?;
        self.flights
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&slug);
        Ok(BoxAnswer::Done)
    }

    /// Records the work place as the Box's baseline.
    async fn seal(&self, slug: &str) -> Result<BoxAnswer, String> {
        let (slug, _flight) = self.flight(slug).await?;
        let Some(mut held) = self.held(&slug)? else {
            return Ok(BoxAnswer::Gone);
        };
        match self.provider.seal(&slug).await {
            Err(ProvideError::Missing) => return Ok(BoxAnswer::Gone),
            other => other.map_err(|e| e.to_string())?,
        }
        held.sealed = true;
        self.keep(&slug, &held)?;
        Ok(BoxAnswer::Done)
    }

    /// Makes `env` the Box's environment, in both places, from now on.
    async fn env(&self, slug: &str, env: Env) -> Result<BoxAnswer, String> {
        if let Some((name, _)) = env.vars.iter().find(|(name, _)| !valid_name(name)) {
            return Err(format!("{name:?} is not a variable name"));
        }
        let (slug, _flight) = self.flight(slug).await?;
        let Some(mut held) = self.held(&slug)? else {
            return Ok(BoxAnswer::Gone);
        };
        match self.provider.apply_env(&slug, &env).await {
            Err(ProvideError::Missing) => return Ok(BoxAnswer::Gone),
            other => other.map_err(|e| e.to_string())?,
        }
        held.box_env = env;
        self.keep(&slug, &held)?;
        Ok(BoxAnswer::Done)
    }

    /// Each Box held, or each of `controller`'s.
    async fn list(&self, controller: Option<&ControllerId>) -> BoxAnswer {
        BoxAnswer::Listed {
            boxes: self.reports_of(controller).await,
        }
    }

    /// Every Box's health, for the report the agent sends the runner.
    pub(crate) async fn reports(&self) -> Vec<BoxReport> {
        self.reports_of(None).await
    }

    /// The health of each Box held, or each of `controller`'s.
    async fn reports_of(&self, controller: Option<&ControllerId>) -> Vec<BoxReport> {
        let seen = self.provider.observe().await.unwrap_or_default();
        let mut reports = Vec::new();
        for slug in self.store.slugs() {
            if controller.is_some_and(|c| slug.controller().as_ref() != Some(c)) {
                continue;
            }
            if let Ok(Some(held)) = self.store.load(&slug) {
                let observed = seen.iter().find(|o| o.slug == slug);
                let mut said = report(&slug, &held, observed);
                if self.in_flight(&slug) {
                    said.down = None;
                }
                reports.push(said);
            }
        }
        reports
    }

    /// Whether a verb works on `slug` now, which may stop and start its places on purpose.
    fn in_flight(&self, slug: &Slug) -> bool {
        self.flights
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(slug)
            .is_some_and(|lock| lock.try_lock().is_err())
    }

    /// `slug` when the Box is held and awake; none when it is not held; an error when it sleeps.
    fn awake(&self, slug: &str) -> Result<Option<Slug>, String> {
        let slug = Slug::parse(slug).ok_or_else(|| format!("{slug:?} is not a Box slug"))?;
        match self.held(&slug)? {
            None => Ok(None),
            Some(held) if held.want == Want::Asleep => Err("the Box is asleep".to_owned()),
            Some(_) => Ok(Some(slug)),
        }
    }
}

/// One Box's health from what is held and what the provider sees.
fn report(slug: &Slug, held: &Held, seen: Option<&Observed>) -> BoxReport {
    let up = seen.is_some_and(|o| o.agent == PlaceState::Up && o.work == PlaceState::Up);
    let state = match (&held.broken, held.want) {
        (Some(why), _) => ReportState::Broken { why: why.clone() },
        (None, Want::Asleep) => ReportState::Asleep,
        (None, Want::Awake) if up => ReportState::Awake,
        (None, Want::Awake) => ReportState::Mending,
    };
    let down = match (&state, seen) {
        (ReportState::Mending, Some(o)) => {
            let mut said: Vec<String> = [o.agent, o.work]
                .into_iter()
                .filter_map(place_down)
                .collect();
            said.dedup();
            Some(said.join("; "))
        }
        _ => None,
    };
    BoxReport {
        slug: slug.as_str().to_owned(),
        state,
        generation: held.generation,
        oom_kills: seen.map_or(0, |o| o.oom_kills),
        last_mend: None,
        mem_mib: seen.and_then(|o| o.mem_mib),
        down,
    }
}

/// How a place is down, worded for people; none when it runs.
fn place_down(place: PlaceState) -> Option<String> {
    Some(match place {
        PlaceState::Up => return None,
        PlaceState::Down => "the Box's container is not running".to_owned(),
        PlaceState::Exited(Ended { code, oom: true }) => {
            format!("the Box's container ran out of memory (exit {code})")
        }
        PlaceState::Exited(Ended { code, oom: false }) => {
            format!("the Box's container stopped (exit {code})")
        }
    })
}

/// Whether `name` can be exported: `[A-Za-z_][A-Za-z0-9_]*`.
fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// A `refused` answer giving `reason`.
const fn refused(reason: String) -> BoxAnswer {
    BoxAnswer::Refused { reason }
}

/// Writes `said` as an answer line; a closed stream is the controller's to notice.
async fn answer<W: AsyncWrite + Unpin + Send>(stream: &mut W, said: BoxAnswer) {
    let _: io::Result<()> = ops::write_line(stream, &said).await;
}
