//! `webhook-claim` and `webhook-report`: a front with `webhooks: true` leases the users'
//! pending webhook deliveries, posts them, and reports how each ended.

use std::time::Duration;

use moor_core::request::{SecretValue, WebhookClaimReq, WebhookOutcome, WebhookReportReq};
use moor_core::user::Caller;
use moor_core::view::user::{
    WebhookClaimValue, WebhookDeliveryView, WebhookHeader, WebhookReportValue,
};
use moor_core::ErrorKind;
use serde_json::{json, Value};

use super::to_value;
use crate::error::CommandError;
use crate::store::{webhook_header_name, webhook_key_name, ClaimedDelivery, Store};
use crate::vault::Vault;

/// The refusal for any caller but a front whose config sets `webhooks: true`.
pub(crate) const FRONT_ONLY: &str =
    "webhook-claim and webhook-report serve only a front whose controller config sets `webhooks: true`";

/// The `schema` every body names; a change to the shape gets a new one.
const SCHEMA: &str = "moor.notify/1";

/// Most deliveries one claim hands back, so one reply stays small.
const MAX_CLAIM: u32 = 100;

/// Refuses `caller` unless it is a front for which `webhooks` (its config's `webhooks: true`)
/// holds.
pub(crate) fn permit(
    webhooks: impl Fn(&str) -> bool,
    caller: &Result<Caller, String>,
) -> Result<(), CommandError> {
    match caller {
        Ok(Caller::Front(ns)) if webhooks(ns) => Ok(()),
        _ => Err(CommandError::new(ErrorKind::Refused, FRONT_ONLY)),
    }
}

/// `webhook-claim`: lease the oldest pending delivery of each webhook, up to `req.limit`.
pub(crate) fn claim(
    store: &Store,
    vault: &Vault,
    req: &WebhookClaimReq,
) -> Result<Value, CommandError> {
    let lease = Duration::from_millis(req.lease_ms);
    let mut deliveries = Vec::new();
    for row in store.claim_webhook_deliveries(req.limit.min(MAX_CLAIM), lease)? {
        let open = |name, sealed| {
            vault
                .open(row.user, &name, sealed)
                .map(|plain| SecretValue::new(plain.expose().to_vec()))
                .map_err(|e| CommandError::new(ErrorKind::Internal, e.to_string()))
        };
        let key = open(webhook_key_name(row.webhook)?, &row.sealed_key)?;
        let header = match &row.header {
            Some((name, sealed)) => Some(WebhookHeader {
                name: name.clone(),
                value: open(webhook_header_name(row.webhook)?, sealed)?,
            }),
            None => None,
        };
        deliveries.push(WebhookDeliveryView {
            delivery: row.delivery,
            webhook: row.webhook,
            url: row.url.clone(),
            key,
            header,
            event: row.kind.clone(),
            body: body(store, &row)?,
        });
    }
    to_value(&WebhookClaimValue { deliveries })
}

/// `webhook-report`: end the delivery and record its outcome on its webhook.
pub(crate) fn report(store: &Store, req: &WebhookReportReq) -> Result<Value, CommandError> {
    let outcome = match &req.outcome {
        WebhookOutcome::Delivered => Ok(()),
        WebhookOutcome::Failed { reason } => Err(reason.as_str()),
    };
    if !store.report_webhook_delivery(req.delivery, outcome)? {
        return Err(CommandError::new(
            ErrorKind::Refused,
            format!("no pending webhook delivery {}", req.delivery),
        ));
    }
    to_value(&WebhookReportValue {
        delivery: req.delivery,
    })
}

/// The `moor.notify/1` body for `row`, with the Box's state and its agent's newest message as
/// they are now, as `show` with the agent reports them.
fn body(store: &Store, row: &ClaimedDelivery) -> Result<Value, CommandError> {
    let data: Value = serde_json::from_str(&row.payload).unwrap_or(Value::Null);
    let boxed = store.box_row(row.box_id)?;
    let mut body = json!({
        "schema": SCHEMA,
        "delivery": row.delivery,
        "offset": row.event,
        "event": row.kind,
        "ts": row.ts,
        "box": boxed.as_ref().map(|b| b.name.as_str().to_owned()),
        "box_id": row.box_id.get(),
        "session": row.session,
        "data": data,
    });
    if let (Some(boxed), Some(out)) = (boxed, body.as_object_mut()) {
        out.insert("lifecycle".to_owned(), to_value(&boxed.lifecycle)?);
        out.insert("session_state".to_owned(), to_value(&boxed.session)?);
        if let Some(said) = super::show::said_of(store, row.box_id)? {
            out.insert("agent_said".to_owned(), to_value(&said)?);
        }
    }
    Ok(body)
}

#[cfg(test)]
mod tests;
