//! Port tunnels against a fake crabbox whose `tunnel` prints a listener the test holds, which
//! stands for ssh's local end.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    reason = "tests"
)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use moor_client::link::ops::{self, Reply};
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
use tokio::net::TcpListener;

use super::{serve, Table};
use crate::pool::Pool;

const SLUG: &str = "moor-1a2b3c4d-7";

const FAKE: &str = r#"#!/bin/sh
echo "$PWD $*" >> "$HOME/tunnel.log"
[ -e "$HOME/tunnel-fail" ] && { echo "ssh: connect to host failed" >&2; exit 2; }
[ -e "$HOME/tunnel-slow" ] && sleep 3
echo $$ >> "$HOME/tunnel.pids"
echo "http://127.0.0.1:$(cat "$HOME/tunnel.port")"
exec sleep 300
"#;

/// A home with a lease dir and the fake crabbox, whose tunnels listen where `port` says.
fn fixture(port: u16) -> (tempfile::TempDir, Pool) {
    let home = tempfile::tempdir().unwrap();
    let crabbox = home.path().join("crabbox");
    std::fs::write(&crabbox, FAKE).unwrap();
    std::fs::set_permissions(&crabbox, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(home.path().join("tunnel.port"), port.to_string()).unwrap();
    std::fs::create_dir_all(home.path().join("moor-pool").join(SLUG).join("lease")).unwrap();
    let pool = Pool {
        provider: "fake".to_owned(),
        max: None,
        reserve_mib: None,
        cache_gb: None,
        crabbox,
    };
    (home, pool)
}

/// A listener that echoes what it reads, and its port.
async fn echo() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut conn, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buf = [0u8; 256];
                while let Ok(n @ 1..) = conn.read(&mut buf).await {
                    if conn.write_all(&buf[..n]).await.is_err() {
                        break;
                    }
                }
            });
        }
    });
    port
}

/// A listener that closes every connection at once, as a tunnel does for a port nothing serves.
async fn closing() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { while listener.accept().await.is_ok() {} });
    port
}

/// Serves one port op for `port`; the runner's end of the stream and the serve call.
fn ask(
    table: &Arc<Table>,
    (home, pool): (&Path, &Pool),
    port: u16,
    probe: bool,
) -> (DuplexStream, tokio::task::JoinHandle<Option<String>>) {
    let (near, far) = tokio::io::duplex(1 << 16);
    let (table, home, pool) = (Arc::clone(table), home.to_path_buf(), pool.clone());
    let served =
        tokio::spawn(async move { serve(&table, (&home, &pool), far, (SLUG, port), probe).await });
    (near, served)
}

async fn reply(conn: &mut DuplexStream) -> Reply {
    ops::read_line(conn).await.unwrap()
}

fn pids(home: &Path) -> Vec<String> {
    std::fs::read_to_string(home.join("tunnel.pids"))
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

fn alive(pid: &str) -> bool {
    PathBuf::from(format!("/proc/{pid}")).exists()
        && !std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|s| s.contains(") Z"))
}

#[tokio::test]
async fn a_connection_reaches_the_port_and_shares_the_tunnel_with_the_next() {
    let port = echo().await;
    let (home, pool) = fixture(port);
    let table = Arc::new(Table::new(Duration::from_secs(60)));
    let place = (home.path(), &pool);
    let (mut a, _) = ask(&table, place, 5177, false);
    let (mut b, _) = ask(&table, place, 5177, false);
    assert_eq!(reply(&mut a).await, Reply::Started);
    assert_eq!(reply(&mut b).await, Reply::Started);
    a.write_all(b"GET / HTTP/1.1\r\n").await.unwrap();
    let mut got = [0u8; 16];
    a.read_exact(&mut got).await.unwrap();
    assert_eq!(&got, b"GET / HTTP/1.1\r\n");
    assert_eq!(
        pids(home.path()).len(),
        1,
        "one tunnel for both connections"
    );
    let log = std::fs::read_to_string(home.path().join("tunnel.log")).unwrap();
    assert!(
        log.contains("tunnel --provider fake --id moor-1a2b3c4d-7 --local-port 0 5177"),
        "{log}"
    );
    table.stop_all().await;
}

#[tokio::test]
async fn a_probe_says_whether_the_vm_closes_the_connection() {
    let (home, pool) = fixture(echo().await);
    let table = Arc::new(Table::new(Duration::from_secs(60)));
    let (mut live, _) = ask(&table, (home.path(), &pool), 5177, true);
    assert_eq!(reply(&mut live).await, Reply::Reachable);
    table.stop_all().await;

    let (home, pool) = fixture(closing().await);
    let (mut dead, _) = ask(&table, (home.path(), &pool), 5177, true);
    assert_eq!(reply(&mut dead).await, Reply::Unreachable);
    table.stop_all().await;
}

#[tokio::test]
async fn a_tunnel_stops_after_its_last_connection_and_the_next_starts_another() {
    let (home, pool) = fixture(echo().await);
    let table = Arc::new(Table::new(Duration::from_millis(200)));
    let place = (home.path(), &pool);
    let (mut a, served) = ask(&table, place, 5177, false);
    assert_eq!(reply(&mut a).await, Reply::Started);
    let first = pids(home.path());
    drop(a);
    served.await.unwrap();
    assert!(
        alive(&first[0]),
        "the tunnel outlives its connection for the idle time"
    );
    tokio::time::sleep(Duration::from_millis(900)).await;
    assert!(!alive(&first[0]), "and stops after it");
    let (mut b, _) = ask(&table, place, 5177, false);
    assert_eq!(reply(&mut b).await, Reply::Started);
    assert_eq!(pids(home.path()).len(), 2);
    table.stop_all().await;
}

#[tokio::test]
async fn stopping_a_lease_ends_its_tunnels_and_cuts_its_connections() {
    let (home, pool) = fixture(echo().await);
    let table = Arc::new(Table::new(Duration::from_secs(60)));
    let (mut a, _) = ask(&table, (home.path(), &pool), 5177, false);
    assert_eq!(reply(&mut a).await, Reply::Started);
    let pid = pids(home.path())[0].clone();
    table.stop_lease(SLUG).await;
    assert!(!alive(&pid));
    table.stop_lease(SLUG).await;
}

#[tokio::test]
async fn a_failed_tunnel_answers_with_what_crabbox_said() {
    let (home, pool) = fixture(echo().await);
    std::fs::write(home.path().join("tunnel-fail"), "").unwrap();
    let table = Arc::new(Table::new(Duration::from_secs(60)));
    let (mut a, served) = ask(&table, (home.path(), &pool), 5177, false);
    assert_eq!(
        reply(&mut a).await,
        Reply::Refused {
            reason: "ssh: connect to host failed".to_owned()
        }
    );
    assert_eq!(
        served.await.unwrap().as_deref(),
        Some("ssh: connect to host failed")
    );
}

#[tokio::test]
async fn a_lease_forwards_eight_ports_and_carries_sixty_four_connections() {
    let (home, pool) = fixture(echo().await);
    let table = Arc::new(Table::new(Duration::from_secs(60)));
    let place = (home.path(), &pool);
    let mut open = Vec::new();
    for port in 5000..5008 {
        let (mut conn, _) = ask(&table, place, port, false);
        assert_eq!(reply(&mut conn).await, Reply::Started, "port {port}");
        open.push(conn);
    }
    let (mut ninth, _) = ask(&table, place, 5008, false);
    assert_eq!(
        reply(&mut ninth).await,
        Reply::Refused {
            reason: "already forwards 8 ports".to_owned()
        }
    );
    for _ in 8..64 {
        let (mut conn, _) = ask(&table, place, 5000, false);
        assert_eq!(reply(&mut conn).await, Reply::Started);
        open.push(conn);
    }
    let (mut over, _) = ask(&table, place, 5000, false);
    assert_eq!(
        reply(&mut over).await,
        Reply::Refused {
            reason: "already carries 64 connections".to_owned()
        }
    );
    assert_eq!(open.len(), 64);
    table.stop_all().await;
}

#[tokio::test]
async fn a_slug_that_is_not_a_pool_lease_is_refused_before_crabbox_runs() {
    let (home, pool) = fixture(echo().await);
    let table = Arc::new(Table::new(Duration::from_secs(60)));
    let (mut near, far) = tokio::io::duplex(4096);
    let why = serve(&table, (home.path(), &pool), far, ("../etc", 22), false).await;
    assert!(why.is_some());
    assert!(matches!(reply(&mut near).await, Reply::Refused { .. }));
    assert!(!home.path().join("tunnel.log").exists());
}

#[tokio::test]
async fn a_tunnel_with_an_open_connection_is_not_stopped_when_another_ends() {
    let (home, pool) = fixture(echo().await);
    let table = Arc::new(Table::new(Duration::from_millis(200)));
    let place = (home.path(), &pool);
    let (mut a, served) = ask(&table, place, 5177, false);
    let (mut b, _) = ask(&table, place, 5177, false);
    assert_eq!(reply(&mut a).await, Reply::Started);
    assert_eq!(reply(&mut b).await, Reply::Started);
    let pid = pids(home.path())[0].clone();
    drop(a);
    served.await.unwrap();
    tokio::time::sleep(Duration::from_millis(900)).await;
    assert!(alive(&pid), "the second connection still holds the tunnel");
    b.write_all(b"still").await.unwrap();
    let mut got = [0u8; 5];
    b.read_exact(&mut got).await.unwrap();
    assert_eq!(&got, b"still");
    table.stop_all().await;
}

#[tokio::test]
async fn an_old_connections_idle_timer_leaves_a_newer_tunnel_at_the_same_key() {
    let (home, pool) = fixture(echo().await);
    let table = Arc::new(Table::new(Duration::from_millis(300)));
    let place = (home.path(), &pool);
    let (mut a, served) = ask(&table, place, 5177, false);
    assert_eq!(reply(&mut a).await, Reply::Started);
    table.stop_lease(SLUG).await;
    table.allow(SLUG).await;
    let (mut b, _) = ask(&table, place, 5177, false);
    assert_eq!(reply(&mut b).await, Reply::Started);
    let newer = pids(home.path())[1].clone();
    drop(a);
    served.await.unwrap();
    tokio::time::sleep(Duration::from_millis(900)).await;
    assert!(alive(&newer), "the old timer stopped the newer tunnel");
    table.stop_all().await;
}

#[tokio::test]
async fn a_lease_going_down_starts_no_tunnel_until_it_is_up_again() {
    let (home, pool) = fixture(echo().await);
    let table = Arc::new(Table::new(Duration::from_secs(60)));
    let place = (home.path(), &pool);
    table.stop_lease(SLUG).await;
    let (mut refused, _) = ask(&table, place, 5177, false);
    assert_eq!(
        reply(&mut refused).await,
        Reply::Refused {
            reason: "is going to sleep or stop".to_owned()
        }
    );
    assert!(!home.path().join("tunnel.log").exists());
    table.allow(SLUG).await;
    let (mut ok, _) = ask(&table, place, 5177, false);
    assert_eq!(reply(&mut ok).await, Reply::Started);
    table.stop_all().await;
}

#[tokio::test]
async fn a_tunnel_that_is_slow_to_start_blocks_neither_a_stop_nor_another_port() {
    let (home, pool) = fixture(echo().await);
    std::fs::write(home.path().join("tunnel-slow"), "").unwrap();
    let table = Arc::new(Table::new(Duration::from_secs(60)));
    let place = (home.path(), &pool);
    let (mut slow, _) = ask(&table, place, 5177, false);
    tokio::time::sleep(Duration::from_millis(300)).await;
    tokio::time::timeout(Duration::from_secs(1), table.stop_lease(SLUG))
        .await
        .unwrap();
    // The start that was under way finds its lease going down and stops its tunnel.
    assert!(matches!(reply(&mut slow).await, Reply::Refused { .. }));
}

#[tokio::test]
async fn killing_all_signals_every_tunnel_without_waiting() {
    let (home, pool) = fixture(echo().await);
    let table = Arc::new(Table::new(Duration::from_secs(60)));
    let (mut a, _) = ask(&table, (home.path(), &pool), 5177, false);
    assert_eq!(reply(&mut a).await, Reply::Started);
    let pid = pids(home.path())[0].clone();
    table.kill_all_now();
    for _ in 0..100 {
        if !alive(&pid) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(!alive(&pid));
    table.stop_all().await;
}
