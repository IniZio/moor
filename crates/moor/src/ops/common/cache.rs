//! A Box's cache volumes: the plan its acquire carries and its provisioning derives again.

use moor_core::cache::{AgentCache, CacheLayers, CachePlan, Origin};
use moor_core::config::{BoxSettings, DriverConfig, Flags};
use moor_core::domain::cache_save::{CacheSave, CacheSaveDenied};
use moor_core::domain::decision::ExtraEvent;
use moor_core::seam::driver::DriverCaps;
use moor_core::user::UserId;
use moor_core::{ProviderId, Warning};

use crate::cx::Cx;
use crate::ops::common::facts::settings_of;
use crate::ops::common::node::seat_of;
use crate::ops::common::step;
use crate::store::{BoxRow, Store};

/// The feature a machine's agent reports when its crabbox mounts cache volumes.
const CACHE_VOLUME: &str = "cache-volume";
/// The feature a machine's agent reports when a pause can save the lease's cache volumes.
const CACHE_SAVE_ON_PAUSE: &str = "cache-save-on-pause";

/// Why a Box whose plan holds volumes starts without them.
#[derive(Clone, Copy)]
enum Why {
    /// The operator turned cache volumes off for the provider.
    SwitchedOff,
    /// The Box has no owning user to key the volumes to.
    NoUser,
    /// The provider keeps no cache volumes.
    Provider,
    /// The personal machine is not linked and online as the owner's.
    MachineOffline,
    /// The personal machine's agent does not report cache volumes.
    MachineLacks,
}

impl Why {
    /// The name the `cache_cold` event carries.
    const fn name(self) -> &'static str {
        match self {
            Self::SwitchedOff => "switched_off",
            Self::NoUser => "no_user",
            Self::Provider => "provider_unsupported",
            Self::MachineOffline => "machine_offline",
            Self::MachineLacks => "machine_lacks_feature",
        }
    }

    /// The reason in a sentence.
    fn detail(self, provider: &ProviderId) -> String {
        match self {
            Self::SwitchedOff => format!(
                "cache volumes are switched off for provider {}",
                provider.as_str()
            ),
            Self::NoUser => "the Box has no owning user".to_owned(),
            Self::Provider => format!("provider {} keeps no cache volumes", provider.as_str()),
            Self::MachineOffline => {
                "the machine is not linked and online as the owner's".to_owned()
            }
            Self::MachineLacks => "the machine does not report cache volumes".to_owned(),
        }
    }

    /// Whether volumes the repo or operator declared warn when this keeps them out; the operator's
    /// own switch and a Box with no user do not.
    const fn warns(self) -> bool {
        !matches!(self, Self::SwitchedOff | Self::NoUser)
    }
}

/// Why a Box gets no cache volumes.
enum Cold {
    /// It would hold volumes but one reason keeps them out.
    Off {
        /// The reason.
        why: Why,
        /// How many of the volumes the repo or the operator declared.
        own: usize,
    },
    /// The declarations clash, or the salt could not be read.
    Refused(String),
}

/// The Box's cache layers: its settings, and the agent's install directory when moor installs the
/// agent.
fn layers(cx: &Cx, cfg: &BoxSettings) -> CacheLayers {
    let agent = cx
        .engine()
        .agent_install()
        .filter(|_| cfg.install_agent)
        .map(|i| AgentCache {
            dirs: i.dirs,
            check: i.check,
        });
    CacheLayers {
        agent,
        ..cfg.cache.clone()
    }
}

/// Why the personal machine `row` is placed on keeps no crabbox cache volumes for `user`, when
/// it does not: it is offline, unlinked or not the owner's, or its agent does not report them.
fn machine_lacks(cx: &Cx, store: &Store, row: &BoxRow, user: UserId) -> Option<Why> {
    match machine_features(cx, store, row, user) {
        None => Some(Why::MachineOffline),
        Some(features) if !features.iter().any(|f| f == CACHE_VOLUME) => Some(Why::MachineLacks),
        Some(_) => None,
    }
}

/// What the online agent of the owner's personal machine `row` is placed on reports.
fn machine_features(cx: &Cx, store: &Store, row: &BoxRow, user: UserId) -> Option<Vec<String>> {
    let seat = seat_of(cx, store, row).ok()??;
    let links = store.user_machine_links(user).unwrap_or_default();
    let (_, link) = links
        .into_iter()
        .find(|(_, l)| l.machine_id == seat.machine)?;
    link.online.then_some(link.features)
}

/// Ok when the provider under `row` can save cache volumes while the Box sleeps; why not otherwise.
pub(crate) fn saves_on_pause(cx: &Cx, store: &Store, row: &BoxRow) -> Result<(), CacheSaveDenied> {
    let boxed = row.boxed.as_ref().ok_or(CacheSaveDenied::NoFeature)?;
    let reports = if boxed.provider.host().is_some() {
        row.user
            .and_then(|user| machine_features(cx, store, row, user))
            .ok_or(CacheSaveDenied::MachineOffline)?
            .iter()
            .any(|f| f == CACHE_SAVE_ON_PAUSE)
    } else {
        cx.driver()
            .caps(&boxed.provider)
            .is_ok_and(|caps| caps.cache_save_on_pause)
    };
    reports.then_some(()).ok_or(CacheSaveDenied::NoFeature)
}

/// What a sleep of `row` decides about its caches, where `settled` says whether its agent's work is
/// done. A Box with no volumes on a provider that cannot save any has nothing to decide.
pub(crate) fn save_decision(
    cx: &Cx,
    store: &Store,
    row: &BoxRow,
    settled: Result<(), CacheSaveDenied>,
) -> Option<CacheSave> {
    let denied = |reason| CacheSave::Denied { reason };
    match saves_on_pause(cx, store, row) {
        Ok(()) => Some(settled.map_or_else(denied, |()| CacheSave::Allowed)),
        Err(CacheSaveDenied::NoFeature) if !holds_volumes(cx, store, row) => None,
        Err(reason) => Some(denied(reason)),
    }
}

/// Whether `row`'s plan holds any volumes.
fn holds_volumes(cx: &Cx, store: &Store, row: &BoxRow) -> bool {
    let Some(boxed) = row.boxed.as_ref() else {
        return false;
    };
    settings_of(cx, row, &Flags::default()).is_ok_and(|cfg| {
        !derived(cx, store, row, &cfg, &boxed.provider)
            .volumes()
            .is_empty()
    })
}

/// The plan for `row` on `provider`, or why the Box starts cold. A Box with no volumes to keep is
/// not cold.
fn plan(
    cx: &Cx,
    store: &Store,
    row: &BoxRow,
    cfg: &BoxSettings,
    provider: &ProviderId,
    caps: DriverCaps,
) -> Result<CachePlan, Cold> {
    let drivers = &cx.config().drivers;
    let on = drivers
        .get(provider.as_str())
        .or_else(|| drivers.get(provider.base()))
        .is_none_or(DriverConfig::cache_volumes);
    let user = row.user.filter(|_| on);
    let off = Cold::Off {
        why: if on { Why::NoUser } else { Why::SwitchedOff },
        own: 0,
    };
    let repo = row.repo.as_ref().map(|r| &r.repo);
    let layers = layers(cx, cfg);
    let resolved = match layers.resolve(repo) {
        Ok(resolved) => resolved,
        Err(e) if user.is_some() => return Err(Cold::Refused(e.to_string())),
        Err(_) => return Err(off),
    };
    if resolved.volumes.is_empty() && resolved.dropped.is_empty() {
        return Ok(CachePlan::none());
    }
    let Some(user) = user else {
        return Err(off);
    };
    let lacks = if provider.host().is_some() {
        machine_lacks(cx, store, row, user)
    } else if caps.cache_volumes {
        None
    } else {
        Some(Why::Provider)
    };
    if let Some(why) = lacks {
        let kept = resolved.volumes.iter();
        let own = kept
            .filter(|d| !matches!(d.origin, Origin::Preset | Origin::WorkTree | Origin::Agent))
            .count()
            + resolved.dropped.len();
        return Err(Cold::Off { why, own });
    }
    let refused = |e: &dyn std::fmt::Display| Cold::Refused(e.to_string());
    let salt = store.cache_salt().map_err(|e| refused(&e))?;
    CachePlan::build(user, repo, &layers, &salt).map_err(|e| refused(&e))
}

/// The plan a new Box's acquire carries. A Box that would hold volumes and gets none records a
/// `cache_cold` event saying why, and warns too when the repo or operator declared volumes the
/// provider cannot keep or the declarations clash. Each declared volume the plan dropped warns on
/// its own.
pub(crate) fn for_start(
    cx: &Cx,
    store: &Store,
    row: &BoxRow,
    cfg: &BoxSettings,
    provider: &ProviderId,
    caps: DriverCaps,
) -> CachePlan {
    let cold = match plan(cx, store, row, cfg, provider, caps) {
        Ok(plan) => {
            for d in plan.dropped() {
                cx.warn(Warning {
                    code: "cache_refused".to_owned(),
                    message: d.to_string(),
                });
            }
            return plan;
        }
        Err(cold) => cold,
    };
    let (reason, detail, warning) = match cold {
        Cold::Off { why, own } => {
            let detail = why.detail(provider);
            let warning = (why.warns() && own > 0).then(|| {
                (
                    "cache_unavailable",
                    format!("{detail}; {own} declared volumes skipped, the Box starts cold"),
                )
            });
            (why.name(), detail, warning)
        }
        Cold::Refused(why) => {
            let warning = ("cache_refused", format!("{why}; the Box starts cold"));
            ("refused", why, Some(warning))
        }
    };
    let event = ExtraEvent::CacheCold {
        reason: reason.to_owned(),
        detail,
    };
    if let Err(e) = step::try_record(store, row.id, event) {
        step::warn_unrecorded(cx, &e);
    }
    if let Some((code, message)) = warning {
        cx.warn(Warning {
            code: code.to_owned(),
            message,
        });
    }
    CachePlan::none()
}

/// `row`'s plan on `provider`, derived again as the start derived it, without its warnings;
/// a cold plan when it cannot be.
pub(crate) fn derived(
    cx: &Cx,
    store: &Store,
    row: &BoxRow,
    cfg: &BoxSettings,
    provider: &ProviderId,
) -> CachePlan {
    cx.driver()
        .caps(provider)
        .ok()
        .and_then(|caps| plan(cx, store, row, cfg, provider, caps).ok())
        .unwrap_or_else(CachePlan::none)
}
