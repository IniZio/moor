//! `machine-pair`: the codes machines pair by, kept in the store so a code one gate replica
//! handed out can be confirmed and collected on any other.

use std::time::Duration;

use moor_core::request::{MachinePairReq, PairOp};
use moor_core::user::Caller;
use moor_core::view::user::MachinePairValue;
use moor_core::ErrorKind;
use serde_json::Value;

use super::to_value;
use crate::error::CommandError;
use crate::store::{token_hash, Pairing, Polled, Store};

/// The refusal for any caller but a front.
pub(crate) const FRONT_ONLY: &str = "machine-pair serves only a front";

/// The most codes waiting at once; more are refused until some are used or expire.
const MOST_WAITING: usize = 256;

/// Refuses `caller` unless it is a front.
pub(crate) fn permit(caller: &Result<Caller, String>) -> Result<(), CommandError> {
    match caller {
        Ok(Caller::Front(_)) => Ok(()),
        _ => Err(CommandError::new(ErrorKind::Refused, FRONT_ONLY)),
    }
}

/// Runs one step of a code's life.
pub(crate) fn run(store: &Store, req: &MachinePairReq) -> Result<Value, CommandError> {
    let value = match &req.op {
        PairOp::Start {
            code,
            device_code,
            ask,
            ttl_secs,
        } => {
            let pairing = Pairing {
                code,
                device_hash: &token_hash(device_code.as_bytes()),
                ask: &ask.to_string(),
                ttl: Duration::from_secs(*ttl_secs),
            };
            match store.start_pairing(&pairing, MOST_WAITING)? {
                Some(true) => MachinePairValue::Started,
                Some(false) => MachinePairValue::Taken,
                None => MachinePairValue::Full,
            }
        }
        PairOp::Show { code } => waiting(store.waiting_pairing(code)?)?,
        PairOp::Claim { code } => waiting(store.claim_pairing(code)?)?,
        PairOp::Settle { code, enrolled } => {
            let enrolled = enrolled.as_ref().map(Value::to_string);
            if store.settle_pairing(code, enrolled.as_deref())? {
                MachinePairValue::Settled
            } else {
                MachinePairValue::Gone
            }
        }
        PairOp::Poll { device_code } => {
            match store.poll_pairing(&token_hash(device_code.as_bytes()))? {
                Polled::Pending => MachinePairValue::Pending,
                Polled::Expired => MachinePairValue::Gone,
                Polled::Enrolled(enrolled) => MachinePairValue::Enrolled {
                    enrolled: parse(&enrolled)?,
                },
            }
        }
    };
    to_value(&value)
}

/// `Waiting` with the stored ask, or `Gone` without one.
fn waiting(ask: Option<String>) -> Result<MachinePairValue, CommandError> {
    Ok(match ask {
        Some(ask) => MachinePairValue::Waiting { ask: parse(&ask)? },
        None => MachinePairValue::Gone,
    })
}

/// A JSON column read back.
fn parse(text: &str) -> Result<Value, CommandError> {
    serde_json::from_str(text).map_err(|e| CommandError::new(ErrorKind::Internal, e.to_string()))
}

#[cfg(test)]
mod tests;
