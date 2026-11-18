#![allow(
    clippy::unwrap_used,
    clippy::panic,
    reason = "tests assert on fixed fixtures"
)]

use std::time::Duration;

use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
use tokio::sync::{mpsc, oneshot};

use super::{carry, drive, Channel, Frame, Mode, Offer};

#[test]
fn a_hello_without_an_offer_reads_as_none() {
    let hello = json!({"hello": {"machine_id": 7, "version": "0.1.0"}});
    let frame: Frame = serde_json::from_value(hello).unwrap();
    let Frame::Hello { offer, .. } = frame else {
        panic!("not a hello: {frame:?}");
    };
    assert_eq!(offer, None);
}

#[test]
fn a_hello_with_an_offer_round_trips() {
    let frame = Frame::Hello {
        machine_id: 7,
        version: "0.1.0".to_owned(),
        offer: Some(Box::new(Offer {
            cache_gb: Some(20),
            features: vec!["cache-volume".to_owned()],
            mem_mib: Some(31_232),
            cpus: Some(16),
            reserve_mib: None,
            boxes: None,
        })),
    };
    let text = serde_json::to_string(&frame).unwrap();
    assert!(text.contains("\"cache_gb\":20"), "{text}");
    assert!(text.contains("\"mem_mib\":31232"), "{text}");
    let back: Frame = serde_json::from_str(&text).unwrap();
    assert_eq!(serde_json::to_string(&back).unwrap(), text);
    let Frame::Hello { offer, .. } = back else {
        panic!("not a hello");
    };
    assert_eq!(offer.unwrap().features, ["cache-volume"]);
}

#[test]
fn an_empty_offer_leaves_out_every_field() {
    assert_eq!(serde_json::to_string(&Offer::default()).unwrap(), "{}");
}

#[test]
fn a_report_with_a_field_this_build_does_not_know_still_reads() {
    let opened = json!({"report": {
        "mem_available_mib": 1024,
        "mem_total_mib": 4096,
        "swap_used_mib": 12,
    }});
    let opened: super::Opened = serde_json::from_value(opened).unwrap();
    let super::Opened::Report { report } = opened else {
        panic!("not a report: {opened:?}");
    };
    assert_eq!(report.mem_total_mib, 4096);
}

#[test]
fn a_hello_with_a_field_this_build_does_not_know_still_reads() {
    let hello = json!({"hello": {
        "machine_id": 7, "protocol": 1, "version": "9.9.9", "boot_id": "abc",
    }});
    let frame: Frame = serde_json::from_value(hello).unwrap();
    assert!(matches!(frame, Frame::Hello { machine_id: 7, .. }));
}

#[test]
fn an_offer_from_an_agent_that_still_says_checkpoints_reads_and_drops_them() {
    let hello = json!({"hello": {"machine_id": 7, "version": "0.9.0", "offer": {
        "max_boxes": 4,
        "features": ["workspace-checkpoint", "workspace-fork"],
        "checkpoints": true,
        "checkpoints_off": "docker info failed: x",
    }}});
    let frame: Frame = serde_json::from_value(hello).unwrap();
    let Frame::Hello { offer, .. } = frame else {
        panic!("not a hello: {frame:?}");
    };
    let offer = offer.unwrap();
    assert_eq!(
        serde_json::to_value(&offer).unwrap().get("checkpoints"),
        None
    );
}

#[test]
fn a_report_without_the_per_box_list_reads_as_empty_and_writes_none() {
    let opened = json!({"report": {"mem_available_mib": 1, "mem_total_mib": 2}});
    let opened: super::Opened = serde_json::from_value(opened).unwrap();
    let super::Opened::Report { report } = &opened else {
        panic!("not a report: {opened:?}");
    };
    assert!(report.boxes.is_empty());
    assert!(!serde_json::to_string(&opened).unwrap().contains("boxes"));
}

#[test]
fn a_report_with_the_per_box_list_round_trips() {
    let opened = super::Opened::Report {
        report: super::Report {
            mem_available_mib: 1,
            mem_total_mib: 2,
            psi_some_avg10: None,
            psi_full_avg10: None,
            disk_free_mib: None,
            disk_total_mib: None,
            boxes: vec![super::BoxMemory {
                slug: "moor-0badcafe-7".to_owned(),
                host_memory_mib: 1536,
                resume_memory_mib: None,
            }],
            box_health: Vec::new(),
        },
    };
    let text = serde_json::to_string(&opened).unwrap();
    assert!(text.contains("\"host_memory_mib\":1536"), "{text}");
    assert_eq!(
        serde_json::from_str::<super::Opened>(&text).unwrap(),
        opened
    );
}

#[test]
fn a_report_from_an_older_agent_has_no_resume_memory() {
    let back: super::BoxMemory =
        serde_json::from_str(r#"{"slug":"moor-0a1b2c3d-1","host_memory_mib":0}"#).unwrap();
    assert_eq!(back.resume_memory_mib, None);
    let with = super::BoxMemory {
        resume_memory_mib: Some(6144),
        ..back
    };
    let text = serde_json::to_string(&with).unwrap();
    assert!(text.contains("\"resume_memory_mib\":6144"), "{text}");
}

#[test]
fn an_offer_and_a_report_that_say_nothing_of_boxes_are_what_they_were() {
    let offer = Offer {
        cache_gb: Some(2),
        ..Offer::default()
    };
    assert_eq!(serde_json::to_string(&offer).unwrap(), r#"{"cache_gb":2}"#);
    let report = json!({"report": {"mem_available_mib": 1, "mem_total_mib": 2}});
    let opened: super::Opened = serde_json::from_value(report).unwrap();
    let text = serde_json::to_string(&opened).unwrap();
    assert!(!text.contains("box_health"), "{text}");
}

#[test]
fn an_offer_that_provides_boxes_carries_its_protocol_and_an_old_runner_reads_past_it() {
    let offer = Offer {
        boxes: Some(super::boxes::BoxOffer {
            protocol: super::boxes::PROTOCOL,
            provides: "docker".to_owned(),
        }),
        ..Offer::default()
    };
    let text = serde_json::to_string(&offer).unwrap();
    assert_eq!(text, r#"{"boxes":{"protocol":1,"provides":"docker"}}"#);
    assert_eq!(serde_json::from_str::<Offer>(&text).unwrap(), offer);
}

#[test]
fn box_health_is_carried_raw_whatever_a_later_agent_puts_in_it() {
    let opened = json!({"report": {"mem_available_mib": 1, "mem_total_mib": 2, "box_health": [
        {"slug": "moor-0badcafe-7", "state": "from-the-future", "extra": [1, 2]},
    ]}});
    let opened: super::Opened = serde_json::from_value(opened).unwrap();
    let super::Opened::Report { report } = &opened else {
        panic!("not a report: {opened:?}");
    };
    assert_eq!(report.box_health.len(), 1);
    let text = serde_json::to_string(&opened).unwrap();
    assert!(text.contains("from-the-future"), "{text}");
}

#[test]
fn the_broker_splices_with_a_unit_ask_and_a_tagged_answer() {
    use super::broker::{Ask, Brokered, PoolOp};
    let ask = Ask {
        machine_id: 12,
        op: PoolOp::Boxes,
    };
    assert_eq!(
        serde_json::to_string(&ask).unwrap(),
        r#"{"machine_id":12,"op":{"op":"boxes"}}"#
    );
    assert_eq!(
        serde_json::to_string(&Brokered::Spliced).unwrap(),
        r#"{"broker":"spliced"}"#
    );
    assert_eq!(super::ops::Op::from(PoolOp::Boxes), super::ops::Op::Boxes);
}

/// Both ends of a link whose bytes pass a relay that can go silent without closing, as a
/// tunnel's dead path does.
struct Linked {
    /// A stream the server end opened.
    near: Channel,
    /// The client end of that stream, once near writes.
    far: std::pin::Pin<Box<dyn std::future::Future<Output = Channel> + Send>>,
    /// Why the server end ended.
    server: oneshot::Receiver<String>,
    /// Why the client end ended.
    client: oneshot::Receiver<String>,
    /// Silences the relay for good.
    silence: oneshot::Sender<()>,
    /// What the server end said.
    said: mpsc::UnboundedReceiver<String>,
}

/// Links a server end carried with `keepalive` to a client end carried with `client_keepalive`.
async fn linked(keepalive: Duration, client_keepalive: Duration) -> Linked {
    let (server_io, server_relay) = tokio::io::duplex(64 * 1024);
    let (client_io, client_relay) = tokio::io::duplex(64 * 1024);
    let (silence, silent) = oneshot::channel();
    let (say, said) = mpsc::unbounded_channel();
    tokio::spawn(relay(server_relay, client_relay, silent));
    let url = format!("ws://moor.test{}", super::PATH);
    let (server, client) = tokio::join!(
        super::accept(server_io),
        tokio_tungstenite::client_async(url, client_io)
    );
    let (server, (client, _)) = (server.unwrap(), client.unwrap());
    let (server, client) = (
        carry(server, keepalive, move |line: &str| {
            let _ = say.send(line.to_owned());
        }),
        carry(client, client_keepalive, |_| {}),
    );
    let (ask, asks) = mpsc::channel(1);
    let server_why = driven(server.socket, Mode::Server, asks, drop);
    let (took, mut inbound) = mpsc::channel(1);
    let (no_ask, no_asks) = mpsc::channel(1);
    let take = move |ch| {
        let _kept = &no_ask;
        let _ = took.try_send(ch);
    };
    let client_why = driven(client.socket, Mode::Client, no_asks, take);
    let (opened, channel) = oneshot::channel();
    ask.send(opened).await.unwrap();
    let near = channel.await.unwrap().unwrap();
    // yamux announces a stream with its first bytes, so the far end sees it once near writes.
    let far = async move { inbound.recv().await.unwrap() };
    Linked {
        near,
        far: Box::pin(far),
        server: server_why,
        client: client_why,
        silence,
        said,
    }
}

/// Drives yamux on `socket` in the background; what resolves with why the link ended.
fn driven(
    socket: super::Socket,
    mode: Mode,
    asks: mpsc::Receiver<super::Asked>,
    take: impl FnMut(Channel) + Send + 'static,
) -> oneshot::Receiver<String> {
    let (ended, why) = oneshot::channel();
    tokio::spawn(async move {
        let _ = ended.send(drive(socket, mode, asks, take, std::future::pending()).await);
    });
    why
}

/// Carries bytes both ways between `a` and `b` until `silent` fires, then holds both open unread.
async fn relay(a: DuplexStream, b: DuplexStream, silent: oneshot::Receiver<()>) {
    let (mut a_read, mut a_write) = tokio::io::split(a);
    let (mut b_read, mut b_write) = tokio::io::split(b);
    let carrying = async {
        tokio::join!(
            tokio::io::copy(&mut a_read, &mut b_write),
            tokio::io::copy(&mut b_read, &mut a_write)
        )
    };
    tokio::select! {
        _ = carrying => {}
        Ok(()) = silent => std::future::pending().await,
    }
}

/// Writes `total` bytes to `ch` while reading from it; how many bytes it read.
async fn swap(ch: Channel, total: usize) -> usize {
    let (mut read, mut write) = tokio::io::split(ch);
    let writing = async move {
        let chunk = vec![7u8; 32 * 1024];
        let mut sent = 0;
        while sent < total {
            if write.write_all(&chunk).await.is_err() {
                return;
            }
            sent += chunk.len();
        }
        let _ = write.shutdown().await;
    };
    let reading = async move {
        let mut buf = vec![0u8; 32 * 1024];
        let mut got = 0;
        while let Ok(n @ 1..) = read.read(&mut buf).await {
            got += n;
        }
        got
    };
    tokio::join!(writing, reading).1
}

const KEEPALIVE: Duration = Duration::from_millis(200);

#[tokio::test]
async fn a_link_carrying_bulk_both_ways_keeps_moving_and_stays_up() {
    let linking = linked(KEEPALIVE, KEEPALIVE);
    let mut l = tokio::time::timeout(Duration::from_secs(5), linking)
        .await
        .unwrap();
    let total: usize = std::env::var("BULK_MB").map_or(64, |v| v.parse().unwrap()) * 1024 * 1024;
    let far = l.far;
    tokio::select! {
        got = async { tokio::join!(swap(l.near, total), async { swap(far.await, total).await }) } => {
            assert_eq!(got, (total, total));
        }
        why = &mut l.server => panic!("the link ended mid-transfer: {why:?}"),
        () = tokio::time::sleep(Duration::from_secs(30)) => panic!("the transfer stalled"),
    }
}

#[tokio::test]
async fn an_idle_link_stays_up_even_when_one_end_pings_three_times_as_seldom() {
    let mut l = linked(KEEPALIVE, KEEPALIVE * 3).await;
    tokio::time::sleep(KEEPALIVE * 10).await;
    assert!(l.server.try_recv().is_err(), "the server end ended");
    assert!(l.client.try_recv().is_err(), "the client end ended");
    l.near.write_all(b"still here").await.unwrap();
    let mut far = l.far.await;
    let mut got = [0u8; 10];
    far.read_exact(&mut got).await.unwrap();
    assert_eq!(&got, b"still here");
    assert!(l.said.try_recv().is_err(), "a healthy link says nothing");
}

#[tokio::test]
async fn both_ends_close_within_three_keepalives_once_the_path_goes_silent_mid_transfer() {
    let Linked {
        near,
        far,
        server,
        client,
        silence,
        mut said,
    } = linked(KEEPALIVE, KEEPALIVE).await;
    tokio::spawn(swap(near, usize::MAX));
    tokio::spawn(async move { swap(far.await, usize::MAX).await });
    tokio::time::sleep(KEEPALIVE * 2).await;
    silence.send(()).unwrap();
    let within = KEEPALIVE * 3 + KEEPALIVE;
    let ended = tokio::time::timeout(within, async { tokio::join!(server, client) }).await;
    let (server, client) = ended.expect("an end still holds the silent link");
    assert!(server.is_ok() && client.is_ok());
    let said: Vec<String> = std::iter::from_fn(|| said.try_recv().ok()).collect();
    assert_eq!(
        said,
        [
            "heard nothing on the link for 0.4s",
            "heard nothing on the link for 0.6s; closing it"
        ]
    );
}
