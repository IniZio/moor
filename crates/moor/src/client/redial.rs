//! How the CLI dials again a controller that did not run its request.

use std::time::Duration;

use moor_client::{redial, Attempt, GaveUp, Redial, WallClock};

use super::{ClientError, Target};
use crate::env::Env;

/// Seconds a request that did not run is dialled again for; `0` turns redialing off.
const REDIAL_VAR: &str = "MOOR_CONTROLLER_REDIAL_SECS";

/// The redial policy: [`REDIAL_VAR`] seconds when it is set, else the default.
fn redial_policy(env: &Env) -> Result<Redial, ClientError> {
    let Some(raw) = env.get(REDIAL_VAR).filter(|v| !v.is_empty()) else {
        return Ok(Redial::default());
    };
    let secs = raw.trim().parse().map_err(|_| {
        ClientError::Usage(format!(
            "{REDIAL_VAR} must be a whole number of seconds, got {raw:?}"
        ))
    })?;
    Ok(Redial::within(Duration::from_secs(secs)))
}

/// Runs `attempt` under the redial policy, telling `notice` once when it starts to wait.
pub(super) fn redialing<T>(
    env: &Env,
    target: &Target,
    mut notice: impl FnMut(&str),
    attempt: impl FnMut() -> Attempt<T, ClientError>,
) -> Result<T, ClientError> {
    let policy = redial_policy(env)?;
    let mut told = false;
    redial(&policy, &WallClock, attempt, |pause, e| {
        if !told {
            told = true;
            notice(&format!("{e}; dialing again for up to {:?}", policy.budget));
        }
        std::thread::sleep(pause);
    })
    .map_err(|e| gave_up(target, e))
}

/// The error a redial run ended with; a spent budget reads as an unreachable controller.
fn gave_up(target: &Target, e: GaveUp<ClientError>) -> ClientError {
    match e {
        GaveUp::Failed(e) => e,
        GaveUp::Budget { budget, last } => {
            let last = match last {
                ClientError::Unreachable { detail, .. } => detail,
                ClientError::Protocol(msg) | ClientError::Usage(msg) => msg,
            };
            ClientError::Unreachable {
                target: target.clone(),
                detail: format!("not back after {budget:?} of redialing; last: {last}"),
            }
        }
    }
}
