//! The delivery worker: claim queued deliveries from the controller, post each, and report
//! how it ended.
//!
//! The controller hands out at most one delivery per webhook at a time, so deliveries to
//! different webhooks run on their own threads and one webhook's stay in order.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use moor_client::Client;
use moor_core::request::{Request, WebhookClaimReq, WebhookOutcome, WebhookReportReq};
use moor_core::response::{ExitClass, Response};
use moor_core::view::user::{WebhookClaimValue, WebhookDeliveryView};

use crate::deliver::{with_retries, Endpoint, Retry};
use crate::guard::Reach;
use crate::Log;

/// First wait before asking the controller again after a failure.
const BACKOFF_MIN: Duration = Duration::from_millis(250);

/// Longest wait between asks after failures.
const BACKOFF_MAX: Duration = Duration::from_secs(10);

/// How the worker runs.
#[derive(Debug, Clone, Copy)]
pub struct Options {
    /// Wait before claiming again after a claim found nothing.
    pub poll: Duration,
    /// How long a claimed delivery stays ours; it must outlast the worst-case retries.
    pub lease: Duration,
    /// Most deliveries in flight at once.
    pub in_flight: usize,
    /// How each delivery is retried.
    pub retry: Retry,
    /// Which addresses a delivery may connect to.
    pub reach: Reach,
}

impl Default for Options {
    /// Poll every second; a ten minute lease over eight attempts of 15 s plus their waits.
    fn default() -> Self {
        Self {
            poll: Duration::from_secs(1),
            lease: Duration::from_secs(600),
            in_flight: 32,
            retry: Retry::default(),
            reach: Reach::Public,
        }
    }
}

/// Why a request to the controller did not give a value.
enum Failed {
    /// The controller could not be reached or spoke out of turn; asking again may work.
    Down(String),
    /// The controller answered and refused.
    Refused(String),
}

/// Claims and delivers until the controller refuses the front.
///
/// # Errors
/// The controller refused a claim, such as when the front's config lacks `webhooks: true`.
pub fn run(client: &Client, options: &Options, log: &Log) -> Result<(), String> {
    let busy = Arc::new(AtomicUsize::new(0));
    let mut backoff = Backoff::default();
    loop {
        let room = options
            .in_flight
            .saturating_sub(busy.load(Ordering::SeqCst));
        if room == 0 {
            thread::sleep(options.poll);
            continue;
        }
        let claim = Request::WebhookClaim(WebhookClaimReq {
            limit: u32::try_from(room).unwrap_or(u32::MAX),
            lease_ms: u64::try_from(options.lease.as_millis()).unwrap_or(u64::MAX),
        });
        let claimed = ask(client, claim).and_then(|value| {
            serde_json::from_value::<WebhookClaimValue>(value.unwrap_or_default())
                .map_err(|e| Failed::Down(format!("unreadable claim: {e}")))
        });
        match claimed {
            Err(Failed::Refused(why)) => return Err(why),
            Err(Failed::Down(why)) => backoff.fail(&why, log),
            Ok(claimed) => {
                backoff.ok();
                if claimed.deliveries.is_empty() {
                    thread::sleep(options.poll);
                }
                for delivery in claimed.deliveries {
                    busy.fetch_add(1, Ordering::SeqCst);
                    let (client, log, busy) = (client.clone(), Arc::clone(log), Arc::clone(&busy));
                    let options = *options;
                    thread::spawn(move || {
                        deliver(&client, &delivery, &options, &log);
                        busy.fetch_sub(1, Ordering::SeqCst);
                    });
                }
            }
        }
    }
}

/// Posts `delivery` with retries and reports the outcome until the controller takes it.
fn deliver(client: &Client, delivery: &WebhookDeliveryView, options: &Options, log: &Log) {
    let what = format!("{} (delivery {})", delivery.event, delivery.delivery);
    let outcome = match serde_json::to_vec(&delivery.body) {
        Err(e) => WebhookOutcome::Failed {
            reason: format!("unserializable body: {e}"),
        },
        Ok(body) => {
            let endpoint = Endpoint::new(
                delivery.url.clone(),
                delivery.key.secret().expose().to_vec(),
                options.reach,
            );
            let posted = with_retries(
                options.retry,
                || endpoint.post(&delivery.event, delivery.delivery, &body, now()),
                &thread::sleep,
            );
            match posted {
                Ok(_) => {
                    log(&format!("posted {what} to {}", endpoint.shown()));
                    WebhookOutcome::Delivered
                }
                Err(reason) => {
                    log(&format!(
                        "gave up on {what} to {}: {reason}",
                        endpoint.shown()
                    ));
                    WebhookOutcome::Failed { reason }
                }
            }
        }
    };
    let mut backoff = Backoff::default();
    loop {
        let report = Request::WebhookReport(WebhookReportReq {
            delivery: delivery.delivery,
            outcome: outcome.clone(),
        });
        match ask(client, report) {
            Ok(_) => return,
            Err(Failed::Refused(why)) => {
                log(&format!("controller refused the report of {what}: {why}"));
                return;
            }
            Err(Failed::Down(why)) => backoff.fail(&why, log),
        }
    }
}

/// Waits longer after each failure and logs a failure only when it differs from the last.
#[derive(Default)]
struct Backoff {
    /// The wait before the next ask; zero before a first failure.
    wait: Duration,
    /// The last failure logged.
    last: Option<String>,
}

impl Backoff {
    /// Logs `why` if it is new, then sleeps.
    fn fail(&mut self, why: &str, log: &Log) {
        if self.last.as_deref() != Some(why) {
            log(&format!("controller: {why}"));
            self.last = Some(why.to_owned());
        }
        self.wait = if self.wait.is_zero() {
            BACKOFF_MIN
        } else {
            self.wait.saturating_mul(2).min(BACKOFF_MAX)
        };
        thread::sleep(self.wait);
    }

    /// Forgets earlier failures.
    fn ok(&mut self) {
        *self = Self::default();
    }
}

/// Runs `request` as the front itself, which asserts no identity.
fn ask(client: &Client, request: Request) -> Result<Option<serde_json::Value>, Failed> {
    let response: Response = client
        .request(request, |_| {})
        .map_err(|e| Failed::Down(e.to_string()))?;
    match response.class {
        ExitClass::Ok => Ok(response.value),
        _ => Err(Failed::Refused(response.error.map_or_else(
            || format!("{} failed", response.cmd),
            |e| e.message,
        ))),
    }
}

/// Now, in unix seconds, for the signed timestamp.
#[expect(
    clippy::disallowed_methods,
    reason = "the signature's timestamp is wall-clock time by definition"
)]
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}
