//! The TCP listeners this process holds, read from procfs.

use std::collections::BTreeSet;

/// `LISTEN` in the `st` column of `/proc/net/tcp`.
const LISTEN: &str = "0A";

/// Socket inodes of the TCP listeners on `port` that this process holds open. An inode names
/// one socket, so a listener that anyone binds later on the same port is not among them.
pub(crate) fn listeners(port: u16) -> BTreeSet<u64> {
    let held: BTreeSet<u64> = std::fs::read_dir("/proc/self/fd")
        .expect("fds")
        .filter_map(|e| std::fs::read_link(e.ok()?.path()).ok())
        .filter_map(|l| {
            l.to_str()?
                .strip_prefix("socket:[")?
                .strip_suffix(']')?
                .parse()
                .ok()
        })
        .collect();
    ["/proc/self/net/tcp", "/proc/self/net/tcp6"]
        .iter()
        .flat_map(|table| {
            std::fs::read_to_string(table)
                .unwrap_or_default()
                .lines()
                .skip(1)
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .filter_map(|row| {
            let cols: Vec<&str> = row.split_whitespace().collect();
            let (_, local) = cols.get(1)?.rsplit_once(':')?;
            let inode = cols.get(9)?.parse().ok()?;
            (u16::from_str_radix(local, 16).ok()? == port
                && *cols.get(3)? == LISTEN
                && held.contains(&inode))
            .then_some(inode)
        })
        .collect()
}

/// Runs `stop` and fails unless it closed every listener this process held on `ports`.
pub(crate) fn assert_closes(ports: &[u16], stop: impl FnOnce()) {
    let before: Vec<(u16, BTreeSet<u64>)> = ports.iter().map(|&p| (p, listeners(p))).collect();
    for (port, held) in &before {
        assert!(
            !held.is_empty(),
            "nothing listens on port {port} before stop"
        );
    }
    stop();
    for (port, held) in before {
        assert!(
            listeners(port).is_disjoint(&held),
            "port {port} still has its listener open"
        );
    }
}
