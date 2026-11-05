//! The store calls a Proxy makes. A Box's supervisor names no Box: each call answers for the
//! Box its hello proved. The shared egress Proxy names the Box in its hello.

use moor_core::domain::event::{Event, MachineEvent};
use moor_core::user::{SecretName, UserId};
use serde::{Deserialize, Serialize};

use crate::store::TunnelAck;

/// One store call.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Call {
    /// The user the live lease `token` of `front` acts for.
    FrontLeaseUser {
        /// The lease token the request presented.
        token: Vec<u8>,
        /// The front it arrived on.
        front: String,
    },
    /// `user`'s secret `name`, while they hold a live lease of `front`.
    FrontSecret {
        /// The front the secret is spent through.
        front: String,
        /// Whose secret.
        user: UserId,
        /// Which secret.
        name: SecretName,
    },
    /// The Box as its supervisor sees it.
    View,
    /// The proxy row's Box port.
    BoxPort,
    /// Takes the lease for `owner`.
    ClaimLease {
        /// The lease owner token.
        owner: String,
        /// How old a rival's heartbeat must be before it is taken over, in milliseconds.
        lease_ms: u64,
    },
    /// Extends `owner`'s lease.
    Heartbeat {
        /// The lease owner token.
        owner: String,
    },
    /// Gives up `owner`'s lease.
    Release {
        /// The lease owner token.
        owner: String,
    },
    /// Marks the proxy ready while `owner` holds the lease.
    MarkReady {
        /// The lease owner token.
        owner: String,
        /// The Box port it serves.
        box_port: u16,
        /// Its process id.
        pid: u32,
        /// The Box handle it serves.
        handle: String,
        /// Its host boot.
        boot_id: Option<String>,
    },
    /// Whether a tunnel was ever opened into the Box.
    TunnelOpened,
    /// Records that a tunnel is about to be opened into the Box.
    NoteTunnelOpened,
    /// The latest tunnel request.
    TunnelRequest,
    /// The latest tunnel answer.
    TunnelAck,
    /// Records the answer to a tunnel request.
    AckTunnel {
        /// The answer.
        ack: TunnelAck,
    },
    /// Records a tunnel restart while `owner` holds the lease.
    TunnelRestarted {
        /// The lease owner token.
        owner: String,
        /// The Box port it reopened.
        box_port: u16,
        /// Why it restarted.
        reason: String,
    },
    /// Records a machine event while `owner` holds the lease.
    MachineEvent {
        /// The lease owner token.
        owner: String,
        /// The event.
        event: MachineEvent,
    },
    /// The machine the Box holds a slot on.
    BoxMachine,
    /// What the Box holds, as the machine service reads it.
    Hold,
    /// Frees the Box's slot as it sleeps.
    SleepMachine,
    /// Grants the Box's machine want.
    GrantMachine,
    /// Whether the newest machine event already says the Box waits at `position`.
    WaitingSaid {
        /// The place in line.
        position: u32,
    },
    /// Lets go of the Box's machine.
    DetachMachine,
    /// Records a proxy audit event.
    RecordEvent {
        /// The event.
        event: Event,
    },
    /// The status of the open limit on credential rule `rule`.
    OpenLimit {
        /// The rule.
        rule: String,
    },
    /// The secret `name` of the Box's user.
    Secret {
        /// Which secret.
        name: SecretName,
    },
    /// Removes the proxy row.
    RemoveProxy,
    /// The spec the Box's Proxy starts from.
    Spec,
    /// The Box's interception CA, made on first call.
    Ca,
    /// The open Box whose egress token hashes to `token_hash`.
    TokenBox {
        /// The hex SHA-256 of the token the Box presented.
        token_hash: String,
    },
}

impl Call {
    /// Whether making the call twice leaves the store as making it once does, so a call that
    /// failed on a connection the controller may have closed can be made again.
    pub(crate) const fn repeatable(&self) -> bool {
        !matches!(
            self,
            Self::MarkReady { .. }
                | Self::TunnelRestarted { .. }
                | Self::MachineEvent { .. }
                | Self::RecordEvent { .. }
                | Self::SleepMachine
                | Self::DetachMachine
        )
    }
}
