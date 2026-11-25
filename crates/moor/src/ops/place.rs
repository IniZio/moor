//! Agent places: repo-less Boxes the agent waits in before any task.
//!
//! A place is started with `moor start --place` by a user on the controller's `agent_places`
//! list. Until a task claims it, no list, event stream, sleep or auto-close sees it; each of those
//! readers asks [`BoxKind::is_idle_place`].
//!
//! The controller's keeper pass keeps one such place for each user on the list. It derives what
//! to do from the stored places, so a restarted controller finds the place that stands and starts
//! no second.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Mutex, PoisonError};

use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::event::close_reason::CloseReason;
use moor_core::domain::hands::HandsState;
use moor_core::domain::place::{BoxKind, PlaceState};
use moor_core::user::{Identity, UserId};
use moor_core::view::StartView;
use moor_core::{BoxId, BoxName, CloseReq, ErrorKind, StartReq};

use super::close;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::store::{BoxRow, Store};

/// A user's newest place and whether a task claimed it; none for a user with no place yet.
type Stamp = Option<(BoxId, bool)>;

/// What the keeper remembers between passes. One per controller.
#[derive(Default)]
pub(crate) struct Keeper {
    /// Each user whose last start left no Box, with their newest place then, so no start is tried
    /// again until it changes.
    held: Mutex<BTreeMap<UserId, Stamp>>,
    /// The places already said to have failed, so each is said once.
    said: Mutex<BTreeSet<BoxId>>,
}

impl Keeper {
    /// Whether this is the first time place `id` is said to have failed.
    fn first_say(&self, id: BoxId) -> bool {
        let mut said = self.said.lock().unwrap_or_else(PoisonError::into_inner);
        said.insert(id)
    }

    /// Notes that `user`'s start left no Box while their newest place was `stamp`.
    fn hold(&self, user: UserId, stamp: Stamp) {
        let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        held.insert(user, stamp);
    }

    /// Whether `user`'s places are as they were when a start of theirs left no Box. A change
    /// lets the next start through.
    fn holds(&self, user: UserId, stamp: Stamp) -> bool {
        let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        if held.get(&user) == Some(&stamp) {
            return true;
        }
        held.remove(&user);
        false
    }
}

/// Starts one place as the user, who acts as the identity.
pub(crate) type Starter<'a> =
    &'a dyn Fn(UserId, &Identity, &StartReq) -> Result<StartView, CommandError>;

/// How an unclaimed place stands, read off its row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Standing {
    /// It is coming up or ready for a task.
    Waiting,
    /// Its setup or its agent's launch failed; it waits for a person.
    Failed,
    /// Its machine is gone or stopped.
    Lost,
}

impl Standing {
    /// How the unclaimed place `row` stands.
    fn of(row: &BoxRow) -> Self {
        if row
            .hands
            .as_ref()
            .is_some_and(|h| h.state == HandsState::Gone)
        {
            Self::Lost
        } else if row.lifecycle == BoxLifecycle::SetupFailed || row.launch_failure.is_some() {
            Self::Failed
        } else if row.asleep() {
            // Only a person or a stop behind moor sleeps an unclaimed place, as the sleep pass
            // never does, and a task would wait for the wake.
            Self::Lost
        } else {
            Self::Waiting
        }
    }
}

/// Whether `newest`, the newest place of a user who has no unclaimed one standing, ended in a
/// rejected start. A person starts the next place then, or a task claims the one that stands.
fn rejected(newest: Option<&BoxRow>) -> bool {
    newest.is_some_and(|row| row.lifecycle == BoxLifecycle::Rejected)
}

/// Keeps one unclaimed place for each user on the `agent_places` list; returns what to log.
///
/// A place whose machine is gone is closed. A user with no unclaimed place standing gets a start,
/// unless their newest place was rejected, or their last start left no Box and nothing has changed
/// since. A failed place stays until a person closes it, and no start replaces it. Each failure
/// is said once. The pass does not retry: the next start comes with the next claim, which leaves
/// the user with no unclaimed place, or with a person's `moor start --place`.
///
/// # Errors
/// A store failure listing the Boxes.
pub(crate) fn keep(
    cx: &Cx,
    keeper: &Keeper,
    start: Starter<'_>,
    draining: &dyn Fn() -> bool,
) -> Result<Vec<String>, CommandError> {
    let store = cx.store()?;
    let mut out = Vec::new();
    for email in cx.env().agent_place_users() {
        if draining() {
            break;
        }
        let Some(user) = store.user_of_email(email)? else {
            continue;
        };
        let Some(identity) = store.identities(user)?.into_iter().next() else {
            continue;
        };
        out.extend(tend(cx, keeper, (user, &identity), (email, start))?);
    }
    Ok(out)
}

/// One user's places: closes the lost, says the failed, and starts one when none stands.
fn tend(
    cx: &Cx,
    keeper: &Keeper,
    (user, identity): (UserId, &Identity),
    (email, start): (&str, Starter<'_>),
) -> Result<Vec<String>, CommandError> {
    let store = cx.store()?;
    let mut out = Vec::new();
    let mut standing = 0;
    for row in store.open_boxes()? {
        if row.user != Some(user) || !row.kind.is_idle_place() {
            continue;
        }
        match Standing::of(&row) {
            Standing::Waiting => standing += 1,
            Standing::Failed => {
                standing += 1;
                if keeper.first_say(row.id) {
                    out.push(failed_line(&row, email));
                }
            }
            Standing::Lost => out.push(close_lost(cx, &row)),
        }
    }
    if standing > 0 {
        return Ok(out);
    }
    let newest = store.newest_place_of(user)?;
    if rejected(newest.as_ref()) {
        if let Some(row) = newest.filter(|row| keeper.first_say(row.id)) {
            out.push(format!(
                "agent place {} of {email} was rejected at start{}; no other starts until a person starts one",
                row.name.as_str(),
                row.state_reason.as_ref().map_or_else(String::new, |why| format!(": {why}")),
            ));
        }
        return Ok(out);
    }
    let stamp = newest.as_ref().map(|row| {
        (
            row.id,
            matches!(row.kind, BoxKind::Place(PlaceState::Claimed(_))),
        )
    });
    if keeper.holds(user, stamp) {
        return Ok(out);
    }
    out.extend(start_one(
        cx,
        keeper,
        (user, identity),
        (email, start),
        stamp,
    )?);
    Ok(out)
}

/// Closes the unclaimed place `row`, whose machine is gone; returns what to log.
fn close_lost(cx: &Cx, row: &BoxRow) -> String {
    let req = CloseReq {
        target: row.id.into(),
        force: true,
    };
    let closed = cx.for_box(row.user, || {
        close::run_with_reason(cx, &req, CloseReason::PlaceLost)
    });
    match closed {
        Ok(_) => format!(
            "closed agent place {}: its machine was lost or stopped",
            row.name.as_str()
        ),
        Err(e) => format!(
            "could not close agent place {}, whose machine was lost: {}",
            row.name.as_str(),
            e.error
        ),
    }
}

/// Starts a place named `place-` and eight hex digits; says how a start that did not make a
/// place that waits went.
fn start_one(
    cx: &Cx,
    keeper: &Keeper,
    (user, identity): (UserId, &Identity),
    (email, start): (&str, Starter<'_>),
    stamp: Stamp,
) -> Result<Vec<String>, CommandError> {
    let store = cx.store()?;
    let name = free_name(cx, store)?;
    let req = StartReq {
        place: true,
        ..StartReq::named(name)
    };
    match start(user, identity, &req) {
        Ok(view) => {
            let id = BoxId::new(view.boxed.id);
            let failed = store
                .box_row(id)?
                .filter(|row| Standing::of(row) == Standing::Failed);
            Ok(failed
                .filter(|row| keeper.first_say(row.id))
                .map(|row| failed_line(&row, email))
                .into_iter()
                .collect())
        }
        Err(e) => {
            keeper.hold(user, stamp);
            // A rejected start leaves a row; its failure is said here, not again by the next pass.
            if let Some(row) = store.newest_place_of(user)?.filter(|r| rejected(Some(r))) {
                keeper.first_say(row.id);
            }
            Ok(vec![format!(
                "could not start an agent place for {email}: {}; no other start is tried until their places change or the controller restarts",
                e.error
            )])
        }
    }
}

/// A place name no open Box has: names are unique across every user.
fn free_name(cx: &Cx, store: &Store) -> Result<BoxName, CommandError> {
    for _ in 0..8 {
        let name = BoxName::parse_allow_bare(&format!("place-{:08x}", cx.rng().u32()))
            .map_err(|e| CommandError::new(ErrorKind::Failed, e.to_string()))?;
        if store.open_box_by_name(&name)?.is_none() {
            return Ok(name);
        }
    }
    Err(CommandError::new(
        ErrorKind::Failed,
        "no free place name after eight draws",
    ))
}

/// What the log says of the failed place `row` of `email`.
fn failed_line(row: &BoxRow, email: &str) -> String {
    let why = row
        .state_reason
        .as_deref()
        .unwrap_or("its agent did not launch");
    format!(
        "agent place {} of {email} failed: {why}; it stays until closed, and no other starts",
        row.name.as_str()
    )
}

/// What a start makes of its Box: a place when it asks for one, else a work Box.
pub(crate) const fn kind_of(req: &StartReq) -> BoxKind {
    if req.place {
        BoxKind::Place(PlaceState::Idle)
    } else {
        BoxKind::Work
    }
}

/// Refuses a place start that names more than a place has, or that a user off the list makes. A
/// start that asks for no place passes.
///
/// # Errors
/// Usage naming the first argument a place does not take; Refused for a user not on the list.
pub(super) fn check_start(
    cx: &Cx,
    store: &Store,
    req: &StartReq,
    user: UserId,
) -> Result<(), CommandError> {
    if !req.place {
        return Ok(());
    }
    let named = [
        ("a repo", req.repo.is_some()),
        ("other repos", req.also.is_some()),
        ("a prompt", req.prompt.is_some()),
        ("an issue", req.issue.is_some()),
        ("a base", req.base.is_some()),
        ("a config", req.config.is_some()),
        ("gates", req.gates.is_some()),
        ("resume", req.resume),
        ("no agent", req.no_agent),
    ];
    if let Some((what, _)) = named.iter().find(|(_, given)| *given) {
        return Err(CommandError::new(
            ErrorKind::Usage,
            format!("a place takes {what} from no one: it starts with no repo and no prompt"),
        ));
    }
    on_roster(cx, store, user)
}

/// Refuses a user who is not on the controller's `agent_places` list. The refusal names the email
/// the controller holds for the user, since a mismatch there is the likely cause.
///
/// # Errors
/// Refused for a user off the list; a store failure.
pub(super) fn on_roster(cx: &Cx, store: &Store, user: UserId) -> Result<(), CommandError> {
    let email = store.email_of(user)?;
    if email
        .as_deref()
        .is_some_and(|email| cx.env().agent_place_user(email))
    {
        return Ok(());
    }
    let held = email.map_or_else(
        || "moor has no email for your user".to_owned(),
        |email| format!("moor has {email} for your user"),
    );
    Err(CommandError::new(
        ErrorKind::Refused,
        format!("you are not on this controller's agent_places list; {held}"),
    ))
}
