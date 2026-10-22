use std::io::Write;
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::thread;
use std::time::Duration;

use super::Hangup;
use crate::env::Cancel;

/// Longer than the work in [`cancelled_after`] waits for a cancel.
const STAY: Duration = Duration::from_millis(1500);

/// Whether `cancel` gets set while work runs on the controller end of a connection after
/// `client` acts on its end.
fn cancelled_after(client: impl FnOnce(UnixStream) + Send + 'static) -> bool {
    let (controller, peer) = UnixStream::pair().unwrap();
    let hangup = Hangup::of(&controller).unwrap();
    let cancel = Cancel::default();
    let acting = thread::spawn(move || client(peer));
    let seen = hangup.cancels(&cancel, || {
        for _ in 0..100 {
            if cancel.is_cancelled() {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        cancel.is_cancelled()
    });
    acting.join().unwrap();
    seen
}

#[test]
fn a_client_that_closes_its_socket_cancels_the_work() {
    assert!(cancelled_after(drop));
}

#[test]
fn a_client_that_shuts_its_sending_side_cancels_the_work() {
    assert!(cancelled_after(|peer| {
        peer.shutdown(Shutdown::Write).unwrap();
        thread::sleep(STAY);
    }));
}

#[test]
fn a_client_that_closes_its_socket_behind_unread_bytes_cancels_the_work() {
    assert!(cancelled_after(|mut peer| {
        peer.write_all(b"next request\n").unwrap();
    }));
}

#[test]
fn a_connected_client_with_unread_bytes_cancels_nothing() {
    assert!(!cancelled_after(|mut peer| {
        peer.write_all(b"next request\n").unwrap();
        thread::sleep(STAY);
    }));
}
