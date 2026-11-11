//! Replacing a controller socket never leaves the path missing for a client dialling it.

use std::io::ErrorKind;
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::controller::bind;

#[test]
fn a_client_dialling_while_a_stale_socket_is_replaced_never_finds_the_path_missing() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("c.sock");
    drop(bind(&sock).unwrap());
    let done = Arc::new(AtomicBool::new(false));
    let dialler = {
        let (sock, done) = (sock.clone(), Arc::clone(&done));
        std::thread::spawn(move || {
            let mut missing = 0;
            while !done.load(Ordering::SeqCst) {
                if let Err(e) = UnixStream::connect(&sock) {
                    if e.kind() == ErrorKind::NotFound {
                        missing += 1;
                    }
                }
            }
            missing
        })
    };
    for _ in 0..2000 {
        drop(bind(&sock).unwrap());
    }
    done.store(true, Ordering::SeqCst);
    assert_eq!(dialler.join().unwrap(), 0);
}
