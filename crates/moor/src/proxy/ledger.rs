//! The ledger: how Proxy processes reach the store, through the controller's ledger socket.
//!
//! A Proxy never opens the store, so one deploy holds one connection pool and no Proxy holds
//! the database URL or the vault key. Each connection opens with a [`Hello`] naming who calls
//! and the key it was started with; every call after it answers for that caller only.
//!
//! Frames are one JSON value per line. The client sends its hello and reads `Ok` or a
//! [`Fault`], then sends one [`Call`] at a time and reads `Ok` with the call's value or a
//! [`Fault`].

mod boxes;
mod call;
mod client;
mod hold;
mod serve;
#[cfg(test)]
pub(crate) mod tests;

use std::fmt;

use moor_core::BoxId;
use serde::{Deserialize, Serialize};

pub(crate) use boxes::{BoxLedger, BoxView, MachineWant};
use call::Call;
pub(crate) use client::FrontsLedger;
pub(crate) use serve::{serve, Keys};

use crate::store::StoreError;

/// The ledger protocol this build speaks; a hello at any other is refused.
pub(crate) const LEDGER: u32 = 1;

/// Who a connection speaks for.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Hello {
    /// A Box's supervisor, with the key the controller started it with.
    Box {
        /// The protocol it speaks.
        v: u32,
        /// The Box it serves.
        box_id: BoxId,
        /// Its key.
        key: String,
    },
    /// The front Proxy, with the key the controller started it with.
    Fronts {
        /// The protocol it speaks.
        v: u32,
        /// Its key.
        key: String,
    },
}

/// Why the controller did not answer a hello or a call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Fault {
    /// The caller may not make it.
    Refused(String),
    /// The store failed.
    Store(String),
}

impl From<StoreError> for Fault {
    fn from(e: StoreError) -> Self {
        Self::Store(e.to_string())
    }
}

/// Why a ledger call failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LedgerError {
    /// The controller could not be reached or went away mid-call.
    Unavailable(String),
    /// The controller refused the caller or the call.
    Refused(String),
    /// The controller's store failed.
    Store(String),
}

impl From<Fault> for LedgerError {
    fn from(f: Fault) -> Self {
        match f {
            Fault::Refused(m) => Self::Refused(m),
            Fault::Store(m) => Self::Store(m),
        }
    }
}

impl fmt::Display for LedgerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable(m) => write!(f, "the controller is unavailable: {m}"),
            Self::Refused(m) => write!(f, "the controller refused: {m}"),
            Self::Store(m) => write!(f, "the store failed: {m}"),
        }
    }
}
