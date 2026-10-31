//! Where a Box runs: the nodes the scheduler sees, and admitting a Box onto the one it picks.

use std::collections::BTreeMap;

use moor_core::box_key::PoolSeat;
use moor_core::config::{BoxSettings, Flags, NodeConfig, UserConfig};
use moor_core::decide::schedule::{schedule, BoxSpec, Node, Scope, Verdict};
use moor_core::domain::box_state::BoxLifecycleInput;
use moor_core::domain::decision::{Decision, Field, Fields};
use moor_core::name::NodeName;
use moor_core::response::Warning;
use moor_core::seam::driver::DriverCaps;
use moor_core::user::UserId;
use moor_core::{ErrorKind, ProviderId};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::facts::{provider_caps, provider_of, require_reverse_tunnel, resolved};
use crate::store::{BoxRow, DriverRow, Store};

/// The shared nodes and the default one. A config without `nodes:` gets one shared node built
/// from `box.provider`, named for it, limited by `drivers.<provider>.max_boxes` or `box.max_boxes`.
///
/// # Errors
/// No node is configured and `box.provider` is unset, or several are and `box.node` is unset.
pub(crate) fn configured(
    user: &UserConfig,
    settings: &BoxSettings,
) -> Result<(BTreeMap<String, NodeConfig>, NodeName), CommandError> {
    if user.nodes.is_empty() {
        let provider = provider_of(settings)?;
        let name = NodeName::parse(provider).unwrap_or_else(|_| name("default"));
        let node = NodeConfig {
            runtime: provider.to_owned(),
            labels: Vec::new(),
            max_boxes: (user.drivers.get(provider).and_then(|d| d.max_boxes))
                .or(user.box_.max_boxes),
        };
        return Ok((BTreeMap::from([(name.to_string(), node)]), name));
    }
    let default = user
        .default_node()
        .and_then(|n| NodeName::parse(n).ok())
        .ok_or_else(|| {
            CommandError::new(
                ErrorKind::Refused,
                "box.node is not set, so moor cannot pick among the configured nodes",
            )
        })?;
    Ok((user.nodes.clone(), default))
}

/// A node name known to be valid.
fn name(s: &str) -> NodeName {
    NodeName::parse(s).unwrap_or_else(|e| unreachable!("{s}: {e}"))
}

/// Every node with the Boxes on it, and the default shared node.
///
/// # Errors
/// [`configured`]'s errors, or a store failure.
pub(crate) fn nodes(
    cx: &Cx,
    settings: &BoxSettings,
) -> Result<(Vec<Node>, NodeName), CommandError> {
    let (shared, default) = configured(cx.config(), settings)?;
    Ok((cx.store()?.nodes(&shared)?, default))
}

/// Unix seconds now.
fn now(cx: &Cx) -> i64 {
    cx.clock().now().as_millis() / 1000
}

/// The verdict for a Box of `user` asking `spec`; a Box with no owner sees only shared nodes.
pub(crate) fn verdict(
    cx: &Cx,
    (nodes, default): (&[Node], &NodeName),
    user: Option<UserId>,
    spec: &BoxSpec,
) -> Verdict {
    let Some(user) = user else {
        let shared: Vec<Node> = nodes
            .iter()
            .filter(|n| n.scope == Scope::Shared)
            .cloned()
            .collect();
        return schedule(&shared, UserId::new(0), default, spec, now(cx));
    };
    schedule(nodes, user, default, spec, now(cx))
}

/// The provider id the driver is handed for a Box on `node`: the runtime of a shared node, and
/// `runtime@machine` for a personal one, which the pool driver resolves to that machine.
pub(crate) fn runtime_id(node: &Node) -> ProviderId {
    match node.scope {
        Scope::Shared => ProviderId::new(&node.runtime),
        Scope::Personal { .. } => ProviderId::new(format!("{}@{}", node.runtime, node.name)),
    }
}

/// The machine seat a Box on `node` takes, for a personal node.
fn seat(store: &Store, node: &Node) -> Result<Option<PoolSeat>, CommandError> {
    match node.scope {
        Scope::Shared => Ok(None),
        Scope::Personal { machine_id, .. } => Ok(Some(PoolSeat {
            machine: machine_id,
            controller: store.controller_id()?,
        })),
    }
}

/// The machine seat for `row`'s recorded node, for a Box acquired again on its provider.
///
/// # Errors
/// A store failure.
pub(crate) fn seat_of(
    cx: &Cx,
    store: &Store,
    row: &BoxRow,
    settings: &BoxSettings,
) -> Result<Option<PoolSeat>, CommandError> {
    let Some(want) = store.box_spec(row.id)?.node else {
        return Ok(None);
    };
    let shared = configured(cx.config(), settings).map_or_else(|_| BTreeMap::new(), |(s, _)| s);
    let all = store.nodes(&shared)?;
    all.iter()
        .find(|n| n.name == want)
        .map_or_else(|| Ok(None), |n| seat(store, n))
}

/// A Box admitted onto a node.
pub(crate) struct Bound {
    /// The driver provider id.
    pub(crate) provider: ProviderId,
    /// Its capabilities.
    pub(crate) caps: DriverCaps,
    /// The machine of a personal node.
    pub(crate) seat: Option<PoolSeat>,
}

/// Schedules `row` and, in the transaction that counts its node, admits it with the decision
/// `requested` builds, or leaves it Pending there. `None` when it waits. A fresh Box that no node
/// can host is refused; a Pending one stays Pending.
///
/// # Errors
/// The refusal, a runtime lacking the reverse tunnel, or a store failure.
pub(crate) fn admit(
    cx: &Cx,
    store: &Store,
    (row, fresh): (&BoxRow, bool),
    settings: &BoxSettings,
    requested: impl FnOnce(&ProviderId) -> Decision,
) -> Result<Option<Bound>, CommandError> {
    let spec = store.box_spec(row.id)?;
    let (all, default) = nodes(cx, settings)?;
    let picked = match verdict(cx, (&all, &default), row.user, &spec) {
        Verdict::Fail { reason } if fresh => {
            return Err(CommandError::new(ErrorKind::Refused, reason));
        }
        Verdict::Fail { .. } => return Ok(None),
        Verdict::Bind { node, .. } | Verdict::Pending { node, .. } => node,
    };
    let Some(node) = all.into_iter().find(|n| n.name == picked) else {
        return Ok(None);
    };
    let provider = runtime_id(&node);
    let caps = require_reverse_tunnel(provider.as_str(), provider_caps(cx, provider.as_str())?)?;
    let on_it = BoxSpec {
        labels: spec.labels,
        node: Some(node.name.clone()),
    };
    let mut bound = false;
    store.apply_admitting(row.id, &node.name, |used| {
        let now = Node {
            used,
            ..node.clone()
        };
        match verdict(cx, (&[now], &default), row.user, &on_it) {
            Verdict::Bind { .. } => {
                bound = true;
                Some(requested(&provider))
            }
            Verdict::Pending { reason, .. } => fresh.then(|| {
                let max_boxes = node
                    .max_boxes
                    .map_or(0, |m| u32::try_from(m).unwrap_or(u32::MAX));
                Decision::new(row.id)
                    .lifecycle(BoxLifecycleInput::AtCapacity { max_boxes })
                    .set(Fields {
                        state_reason: Field::Set(reason),
                        ..Fields::default()
                    })
            }),
            Verdict::Fail { .. } => None,
        }
    })?;
    if !bound {
        return Ok(None);
    }
    Ok(Some(Bound {
        seat: seat(store, &node)?,
        provider,
        caps,
    }))
}

/// Before a Box row exists: refuses a start no node can host, and names a personal node the Box
/// will start on.
///
/// # Errors
/// The refusal, a runtime lacking the reverse tunnel, or [`nodes`]'s errors.
pub(crate) fn preflight(
    cx: &Cx,
    user: UserId,
    settings: &BoxSettings,
    spec: &BoxSpec,
) -> Result<Option<String>, CommandError> {
    let (all, default) = nodes(cx, settings)?;
    let picked = match verdict(cx, (&all, &default), Some(user), spec) {
        Verdict::Fail { reason } => return Err(CommandError::new(ErrorKind::Refused, reason)),
        Verdict::Bind { node, .. } | Verdict::Pending { node, .. } => node,
    };
    let Some(node) = all.iter().find(|n| n.name == picked) else {
        return Ok(None);
    };
    let provider = runtime_id(node);
    require_reverse_tunnel(provider.as_str(), provider_caps(cx, provider.as_str())?)?;
    Ok(matches!(node.scope, Scope::Personal { .. })
        .then(|| format!("starting on your node {}", node.name)))
}

/// The node a live Box started before nodes existed was on, by its provider and handle.
fn legacy_node(all: &[Node], default: Option<&NodeName>, held: &DriverRow) -> Option<NodeName> {
    let personal = |keep: &dyn Fn(&Node) -> bool| {
        all.iter()
            .find(|n| matches!(n.scope, Scope::Personal { .. }) && keep(n))
            .map(|n| n.name.clone())
    };
    if let Some(host) = held.provider.host() {
        return personal(&|n| n.name.as_str() == host);
    }
    let handle = held.handle.as_ref().map(|h| h.as_str().to_owned());
    let mut parts = handle.as_deref().unwrap_or_default().split('/');
    if let (Some(_), Some(machine), Some(slug)) = (parts.next(), parts.next(), parts.next()) {
        let machine = machine
            .parse::<i64>()
            .ok()
            .filter(|_| slug.starts_with("moor-"));
        if let Some(machine) = machine {
            return personal(
                &|n| matches!(n.scope, Scope::Personal { machine_id, .. } if machine_id == machine),
            );
        }
    }
    let same: Vec<&Node> = all
        .iter()
        .filter(|n| n.scope == Scope::Shared && n.runtime == held.provider.as_str())
        .collect();
    match same.as_slice() {
        [only] => Some(only.name.clone()),
        [] => None,
        many => many
            .iter()
            .find(|n| Some(&n.name) == default)
            .map(|n| n.name.clone()),
    }
}

/// Puts each live Box that predates node placement on the node it ran on, so it counts toward
/// that node's capacity. A Box that matches no node stays unplaced, with a warning.
///
/// # Errors
/// A store failure.
pub(crate) fn backfill(cx: &Cx, store: &Store) -> Result<(), CommandError> {
    let settings = resolved(cx, None, &Flags::default());
    let (shared, default) = match configured(cx.config(), &settings) {
        Ok((shared, default)) => (shared, Some(default)),
        Err(_) => (BTreeMap::new(), None),
    };
    let all = store.nodes(&shared)?;
    for (id, held) in store.box_rows_live()? {
        if store.box_spec(id)?.node.is_some() {
            continue;
        }
        match legacy_node(&all, default.as_ref(), &held) {
            Some(node) => {
                store.place_unplaced(id, &node)?;
            }
            None => cx.warn(Warning {
                code: "node_unknown".to_owned(),
                message: format!(
                    "Box {} runs on {}, which matches no node, so it counts toward none",
                    id.get(),
                    held.provider.as_str()
                ),
            }),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
