//! `moor-hc forward`: a port of a Box on this computer's `localhost`, until Ctrl-C.

use std::io::Write;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use moor_client::port_forward::{self, Event, Say, Spec};
use moor_client::session::unix_now;
use moor_client::Client;

use crate::error::Error;

/// Forwards port `port` of Box `name` until SIGINT or SIGTERM, printing each event with its
/// time to stdout.
///
/// # Errors
/// The local port is taken, the controller refuses, the Box closed, or signals cannot be watched.
pub fn run(client: &Client, name: &str, port: u16, local_port: Option<u16>) -> Result<(), Error> {
    let spec = Spec {
        box_name: name.to_owned(),
        port,
        local_port,
    };
    let stop = Arc::new(AtomicBool::new(false));
    for signal in [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM] {
        signal_hook::flag::register(signal, Arc::clone(&stop))?;
    }
    let shown = spec.clone();
    let say: Say = Arc::new(move |event: Event| {
        let _ = writeln!(
            std::io::stdout(),
            "{}  {}",
            time_of_day(unix_now()),
            event.describe(&shown)
        );
    });
    port_forward::run(client, &spec, &stop, say)
        .map(drop)
        .map_err(|e| Error::Refused(e.0))
}

/// `secs` since the epoch as a UTC `HH:MM:SS`.
fn time_of_day(secs: i64) -> String {
    let day = secs.rem_euclid(86_400);
    format!("{:02}:{:02}:{:02}", day / 3600, day / 60 % 60, day % 60)
}

#[cfg(test)]
mod tests {
    use super::time_of_day;

    #[test]
    fn the_time_of_day_is_utc_and_zero_padded() {
        assert_eq!(time_of_day(0), "00:00:00");
        assert_eq!(time_of_day(86_400 + 3 * 3600 + 4 * 60 + 5), "03:04:05");
    }
}
