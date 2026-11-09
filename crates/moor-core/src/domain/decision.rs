//! The single value a store writer applies: inputs, field changes, intent and events.

mod extra;

use super::box_state::BoxLifecycleInput;
use super::cache_save::CacheSave;
use super::hands::{HandsInc, HandsInput};
use super::session_state::SessionInput;
use crate::name::{BoxHandle, BoxId, BoxName, LaunchId, ProviderId, RunId};
use crate::name::{ForgeRepo, GitAuthor};
use crate::user::UserId;

pub use extra::ExtraEvent;

/// A change to one optional column.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Field<T> {
    /// Leave the column as it is.
    #[default]
    Keep,
    /// Write this value.
    Set(T),
    /// Write NULL.
    Clear,
}

/// Changes to a Box's state columns. The columns a Box is started with are in [`SpecFields`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Fields {
    /// Box prompt; a relaunch replaces the one the Box was started with.
    pub prompt: Field<String>,
    /// Agent session id.
    pub agent_session_id: Field<String>,
    /// Reason for the current state.
    pub state_reason: Field<String>,
    /// When setup failed, in milliseconds since the epoch.
    pub setup_failed_at: Field<i64>,
    /// When the setup lines passed and the steps after them began; cleared once those steps ran.
    pub steps_pending_at: Field<i64>,
    /// The recorded launch failure, as JSON.
    pub launch_failure: Field<String>,
}

/// Changes to the facts about a Box's machine.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HandsFields {
    /// Machine provider.
    pub provider: Field<ProviderId>,
    /// Machine handle.
    pub handle: Field<BoxHandle>,
    /// Workdir in the machine.
    pub workdir: Field<String>,
    /// CPUs the machine delivers.
    pub cpus: Field<i64>,
    /// Memory the machine delivers, in MiB.
    pub mem_mib: Field<i64>,
    /// Egress: `none`, `enforced` or `not_enforced`.
    pub egress: Field<String>,
    /// Why the machine cannot sleep, as last reported by the driver.
    pub sleep_reason: Field<String>,
}

/// A write to one incarnation of a Box's hands: the store applies it only while those hands are
/// still the Box's current ones.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandsWrite {
    /// The hands the deciding op read.
    pub inc: HandsInc,
    /// The state machine input.
    pub input: Option<HandsInput>,
    /// Column changes.
    pub fields: HandsFields,
}

/// What a Box is started with. Only a [`NewBox`] carries these, so no later write changes them:
///
/// ```compile_fail
/// use moor_core::domain::decision::{Decision, Field, Fields};
/// use moor_core::name::BoxId;
///
/// let _ = Decision::new(BoxId::new(1)).set(Fields { base: Field::Set("main".into()), ..Fields::default() });
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SpecFields {
    /// Issue reference.
    pub issue: Option<String>,
    /// The prompt the Box starts with.
    pub prompt: Option<String>,
    /// Base ref.
    pub base: Option<String>,
    /// Branch name; a Decision that names the branch by id sets it instead.
    pub branch: Option<String>,
    /// Labels the Box's node must carry.
    pub labels: Vec<String>,
}

/// A recorded in-flight operation, kept so a crash can be recovered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Intent {
    /// A box is being provisioned.
    Provisioning,
    /// A run is executing.
    Running {
        /// The run.
        run: RunId,
    },
    /// Box setup is executing.
    Setup,
    /// A forced close wants the Box gone, ahead of the start it is stopping; a restart honours it.
    Closing,
}

impl Intent {
    /// Returns the stored kind name.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Provisioning => "provisioning",
            Self::Running { .. } => "running",
            Self::Setup => "setup",
            Self::Closing => "closing",
        }
    }

    /// Returns the run for a running intent.
    #[must_use]
    pub const fn run(&self) -> Option<RunId> {
        match self {
            Self::Running { run } => Some(*run),
            Self::Provisioning | Self::Setup | Self::Closing => None,
        }
    }
}

/// A change to the Box intent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum IntentChange {
    /// Leave the intent as it is.
    #[default]
    Keep,
    /// Record this intent.
    Set(Intent),
    /// Remove the intent.
    Clear,
}

/// A Box that does not exist yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewBox {
    /// Box name.
    pub name: BoxName,
    /// The repo the Box clones; `None` for a Box with an empty workdir.
    pub repo: Option<BoxRepo>,
    /// What the Box is started with.
    pub spec: SpecFields,
}

/// A Box's repo with what the forge said about it at the base, before the machine existed.
///
/// The agent controls the Box, so moor never reads these back from it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoxRepo {
    /// The repo on github.com.
    pub repo: ForgeRepo,
    /// The exact `.moor.yaml` text at the base; `None` when the base has no such file.
    pub moor_yaml: Option<String>,
    /// The commit identity the clone sets in the Box.
    pub author: GitAuthor,
    /// The repos listed after this one, in the order the person listed them; empty for a
    /// Box of one repo. This first repo stays primary: it names the branch and holds the
    /// default work tree.
    pub others: Vec<ListedRepo>,
}

/// A Box repo other than its primary one, with its config at the base.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListedRepo {
    /// The repo on github.com.
    pub repo: ForgeRepo,
    /// The exact `.moor.yaml` text at the base; `None` when the base has no such file.
    pub moor_yaml: Option<String>,
}

/// The Box a decision applies to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BoxTarget {
    /// Insert this Box.
    New(Box<NewBox>),
    /// Update this Box.
    Existing(BoxId),
}

/// Everything one store write changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    /// The Box to write.
    pub target: BoxTarget,
    /// Box lifecycle input.
    pub lifecycle_input: Option<BoxLifecycleInput>,
    /// Primary Session input.
    pub session_input: Option<SessionInput>,
    /// State column changes.
    pub set: Fields,
    /// What this decision writes to the Box's hands, and which hands it read.
    pub hands: Option<HandsWrite>,
    /// The agent launch the deciding op read: the store applies the decision only while that
    /// launch is still the Box's live one.
    pub launch: Option<LaunchId>,
    /// Intent change.
    pub intent: IntentChange,
    /// Caller-supplied events appended after the derived ones. Derived state
    /// events cannot be passed here; see [`ExtraEvent`].
    pub events: Vec<ExtraEvent>,
    /// Parent Session of the first Session, for a Decision that inserts a box.
    pub parent_session: Option<i64>,
    /// The user a Box acts for, for a Decision that inserts a box.
    pub requester: Option<UserId>,
    /// Name the inserted Box's branch from its new id, for a Decision that inserts a box.
    pub branch_by_id: bool,
    /// What the sleep this Decision records decided about the caches, for its `asleep` event.
    pub cache_save: Option<CacheSave>,
    /// Why the setup failure this Decision records happened, for its `setup_failed` event.
    pub setup_why: Option<String>,
}

impl Decision {
    /// Starts a decision for an existing box.
    #[must_use]
    pub fn new(box_id: BoxId) -> Self {
        Self::base(BoxTarget::Existing(box_id))
    }

    /// Starts a decision that inserts a box.
    #[must_use]
    pub fn insert(new: NewBox) -> Self {
        Self::base(BoxTarget::New(Box::new(new))).lifecycle(BoxLifecycleInput::Insert)
    }

    /// Builds a decision with no changes.
    fn base(target: BoxTarget) -> Self {
        Self {
            target,
            lifecycle_input: None,
            session_input: None,
            set: Fields::default(),
            hands: None,
            launch: None,
            intent: IntentChange::Keep,
            events: Vec::new(),
            parent_session: None,
            requester: None,
            branch_by_id: false,
            cache_save: None,
            setup_why: None,
        }
    }

    /// Applies the decision only while `launch` is the Box's live agent launch.
    #[must_use]
    pub const fn launch(mut self, launch: LaunchId) -> Self {
        self.launch = Some(launch);
        self
    }

    /// Sets the Box lifecycle input.
    #[must_use]
    pub fn lifecycle(mut self, i: BoxLifecycleInput) -> Self {
        self.lifecycle_input = Some(i);
        self
    }

    /// Sets what the sleep this Decision records decided about the caches.
    #[must_use]
    pub const fn cache_save(mut self, save: Option<CacheSave>) -> Self {
        self.cache_save = save;
        self
    }

    /// Sets why the setup failure this Decision records happened.
    #[must_use]
    pub fn setup_why(mut self, why: Option<String>) -> Self {
        self.setup_why = why;
        self
    }

    /// Sets the primary Session input.
    #[must_use]
    pub const fn session(mut self, i: SessionInput) -> Self {
        self.session_input = Some(i);
        self
    }

    /// Sets the input to the hands `inc`, the ones this decision was made from.
    #[must_use]
    pub fn hands_input(mut self, inc: HandsInc, i: HandsInput) -> Self {
        self.hands_at(inc).input = Some(i);
        self
    }

    /// Sets the state column changes.
    #[must_use]
    pub fn set(mut self, f: Fields) -> Self {
        self.set = f;
        self
    }

    /// Sets the column changes to the hands `inc`, the ones this decision was made from.
    #[must_use]
    pub fn hands(mut self, inc: HandsInc, f: HandsFields) -> Self {
        self.hands_at(inc).fields = f;
        self
    }

    /// The write to the hands `inc`; a decision reads one Box's hands once, so it names one.
    fn hands_at(&mut self, inc: HandsInc) -> &mut HandsWrite {
        let w = self.hands.get_or_insert_with(|| HandsWrite {
            inc,
            input: None,
            fields: HandsFields::default(),
        });
        debug_assert_eq!(w.inc, inc, "one decision names two hands");
        w
    }

    /// The input to the Box's hands, if any.
    #[must_use]
    pub fn input_to_hands(&self) -> Option<&HandsInput> {
        self.hands.as_ref().and_then(|w| w.input.as_ref())
    }

    /// Sets the intent change.
    #[must_use]
    pub const fn intent(mut self, c: IntentChange) -> Self {
        self.intent = c;
        self
    }

    /// Sets the parent Session of the first Session; ignored for an existing box.
    #[must_use]
    pub const fn parent_session(mut self, parent: Option<i64>) -> Self {
        self.parent_session = parent;
        self
    }

    /// Names the inserted Box's branch from the id the store assigns, in the same write as the
    /// row; ignored for an existing box.
    #[must_use]
    pub const fn branch_by_id(mut self) -> Self {
        self.branch_by_id = true;
        self
    }

    /// Sets the user the inserted Box acts for; ignored for an existing box.
    #[must_use]
    pub const fn requester(mut self, user: UserId) -> Self {
        self.requester = Some(user);
        self
    }

    /// Appends an extra event.
    ///
    /// Derived state events (closed, idled, `box_*` ...) are not accepted:
    ///
    /// ```compile_fail
    /// use moor_core::domain::decision::Decision;
    /// use moor_core::domain::event::Event;
    /// use moor_core::domain::box_state::BoxLifecycle;
    /// use moor_core::name::BoxId;
    ///
    /// let _ = Decision::new(BoxId::new(1)).event(Event::Closed { from: BoxLifecycle::Ready, reason: None });
    /// ```
    #[must_use]
    pub fn event(mut self, e: ExtraEvent) -> Self {
        self.events.push(e);
        self
    }
}

#[cfg(test)]
mod tests;
