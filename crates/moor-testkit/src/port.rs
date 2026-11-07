//! Loopback ports that tests hand to another process or bind later.

use std::fs::File;
use std::net::{Ipv4Addr, Ipv6Addr, TcpListener};

/// Ports below the kernel's ephemeral range (32768 and up by default), where a bind to port 0
/// never lands.
const RANGE: std::ops::Range<u16> = 20_000..32_000;

/// A loopback port that no other test reserves while this value lives.
///
/// Dropping a listener frees its port at once, and another test's `bind(0)` can take it before
/// the process that was meant to bind it does. Two things close that gap. The port comes from a
/// range the kernel does not hand out for `bind(0)`. And each port has one lock file in the temp
/// directory, held with an exclusive lock until this value drops, so no other test process picks
/// the same number from the range. The kernel releases the lock when the holder dies.
#[derive(Debug)]
pub struct LockedPort {
    /// The reserved port.
    port: u16,
    /// Holds the file lock for this port.
    _lock: File,
}

impl LockedPort {
    /// Reserves a port that nothing is bound to on either loopback address.
    ///
    /// # Panics
    ///
    /// When every port in the range is locked or in use.
    #[must_use]
    #[allow(
        clippy::expect_used,
        reason = "a test helper with no port to give has nothing to fall back to"
    )]
    pub fn reserve() -> Self {
        RANGE
            .clone()
            .find_map(Self::try_reserve)
            .expect("no free port left in the locked range")
    }

    /// Reserves a port and binds `127.0.0.1` on it.
    ///
    /// # Panics
    ///
    /// When every port in the range is locked or in use.
    #[must_use]
    #[allow(
        clippy::expect_used,
        reason = "a test helper with no port to give has nothing to fall back to"
    )]
    pub fn listen() -> (TcpListener, Self) {
        RANGE
            .clone()
            .find_map(|port| {
                let lock = Self::lock(port)?;
                let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port)).ok()?;
                Some((listener, Self { port, _lock: lock }))
            })
            .expect("no free port left in the locked range")
    }

    /// The reserved port number.
    #[must_use]
    pub const fn port(&self) -> u16 {
        self.port
    }

    /// Locks `port`'s file, or `None` when another holder has it.
    fn lock(port: u16) -> Option<File> {
        let path = std::env::temp_dir().join(format!("moor-testkit-port-{port}.lock"));
        let file = File::create(path).ok()?;
        file.try_lock().ok()?;
        Some(file)
    }

    /// Locks `port` and checks that nothing outside the tests uses it.
    fn try_reserve(port: u16) -> Option<Self> {
        let lock = Self::lock(port)?;
        let v4 = TcpListener::bind((Ipv4Addr::LOCALHOST, port));
        let v6 = TcpListener::bind((Ipv6Addr::LOCALHOST, port));
        let taken = |bound: &std::io::Result<TcpListener>| {
            bound
                .as_ref()
                .is_err_and(|e| e.kind() == std::io::ErrorKind::AddrInUse)
        };
        (!taken(&v4) && !taken(&v6)).then_some(Self { port, _lock: lock })
    }
}

#[cfg(test)]
mod tests {
    use super::LockedPort;

    #[test]
    fn two_live_reservations_never_share_a_port() {
        let held: Vec<LockedPort> = (0..20).map(|_| LockedPort::reserve()).collect();
        let mut ports: Vec<u16> = held.iter().map(LockedPort::port).collect();
        ports.sort_unstable();
        ports.dedup();
        assert_eq!(ports.len(), held.len());
    }
}
