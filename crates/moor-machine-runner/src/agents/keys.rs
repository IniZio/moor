//! The enrolled keys a hello is checked against, read from the controller.

use std::time::Duration;

use moor_client::Client;
use moor_core::request::Request;
use moor_core::view::user::MachineKeysValue;
use ssh_key::PublicKey;

/// How long a hello waits for the controller to check keys, as it does while the controller
/// restarts. Under the agent's 10 s wait for an answer, so the answer still reaches it.
const KEYS_WAIT: Duration = Duration::from_secs(8);
/// The wait between those checks.
const KEYS_RETRY: Duration = Duration::from_millis(250);

/// The key machine `machine_id` was enrolled with, read from the controller each time so a
/// replaced or removed key stops working at once.
pub(super) async fn key_of(client: &Client, machine_id: i64) -> Result<PublicKey, String> {
    let deadline = tokio::time::Instant::now() + KEYS_WAIT;
    let value = loop {
        match machine_keys(client).await {
            Ok(value) => break value,
            Err(e) if tokio::time::Instant::now() + KEYS_RETRY >= deadline => {
                return Err(format!("the server cannot check keys now: {e}"));
            }
            Err(_) => tokio::time::sleep(KEYS_RETRY).await,
        }
    };
    let keys: MachineKeysValue = serde_json::from_value(value)
        .map_err(|e| format!("machine-keys answered something else: {e}"))?;
    let enrolled = keys
        .machines
        .into_iter()
        .find(|m| m.machine_id == machine_id)
        .ok_or_else(|| format!("machine {machine_id} is not enrolled"))?;
    PublicKey::from_openssh(&enrolled.pubkey)
        .map_err(|e| format!("machine {machine_id}'s key does not parse: {e}"))
}

/// The controller's `machine-keys` answer.
async fn machine_keys(client: &Client) -> Result<serde_json::Value, String> {
    let client = client.clone();
    let response =
        tokio::task::spawn_blocking(move || client.request(Request::MachineKeys {}, |_| {}))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
    match response.error {
        Some(error) => Err(error.message),
        None => Ok(response.value.unwrap_or_default()),
    }
}
