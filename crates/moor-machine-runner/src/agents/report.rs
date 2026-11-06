//! The runner's `machine-links` reports to the controller.

use std::time::Duration;

use moor_client::Client;
use moor_core::request::{MachineLinksReq, Request};
use tokio::sync::watch;

/// How long a failed `machine-links` report waits before it is sent again.
const RETRY: Duration = Duration::from_secs(1);
/// How often an unchanged `machine-links` report is sent again, well within the minute after
/// which the controller stops routing to links it has not heard of.
const RENEW: Duration = Duration::from_secs(20);

/// Sends the latest set as `machine-links` after every change and every [`RENEW`], again until
/// the controller takes it.
pub(super) async fn report(client: Client, mut rx: watch::Receiver<MachineLinksReq>) {
    loop {
        if let Ok(Err(_)) = tokio::time::timeout(RENEW, rx.changed()).await {
            return;
        }
        loop {
            let request = Request::MachineLinks(rx.borrow_and_update().clone());
            let client = client.clone();
            let sent = tokio::task::spawn_blocking(move || client.request(request, |_| {})).await;
            if matches!(sent, Ok(Ok(ref r)) if r.error.is_none()) {
                break;
            }
            tokio::time::sleep(RETRY).await;
        }
    }
}
