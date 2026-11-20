//! The App's webhook delivery log, read with the App JWT.

use moor_core::seam::clock::Ts;
use serde::Deserialize;
use serde_json::Value;

use super::{AppClient, PAGE};

/// One entry of the App's webhook delivery log.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct Delivery {
    /// The delivery id, rising with time.
    pub(crate) id: u64,
    /// The event name, such as `pull_request`.
    pub(crate) event: String,
    /// The event's action, such as `opened`.
    pub(crate) action: Option<String>,
}

impl AppClient {
    /// The newest page of the App's webhook delivery log, newest first.
    ///
    /// # Errors
    /// GitHub cannot be reached or refuses the App JWT.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the delivery-log poll reads it")
    )]
    pub(crate) fn deliveries(&self, now: Ts) -> Result<Vec<Delivery>, String> {
        let path = format!("/app/hook/deliveries?per_page={PAGE}");
        self.get(&path, &self.jwt(now)?)
    }

    /// The payload GitHub sent in delivery `id`.
    ///
    /// # Errors
    /// GitHub cannot be reached, refuses the App JWT, or the delivery has no payload.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the delivery-log poll reads it")
    )]
    pub(crate) fn delivery_payload(&self, id: u64, now: Ts) -> Result<Value, String> {
        let path = format!("/app/hook/deliveries/{id}");
        let mut delivery: Value = self.get(&path, &self.jwt(now)?)?;
        match delivery.pointer_mut("/request/payload").map(Value::take) {
            Some(payload) if payload.is_object() => Ok(payload),
            _ => Err(format!("GitHub answered {path} with no payload")),
        }
    }
}
