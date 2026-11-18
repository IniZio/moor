#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::missing_docs_in_private_items,
    reason = "tests assert on fixed fixtures"
)]

use std::sync::Arc;
use std::time::Duration;

use moor_client::link::boxes::{BoxAnswer, BoxOp, ReportState, Size};
use moor_client::link::chan::{read_frame, write_frame, Exit, Frame};
use moor_client::link::ops::{self, Op};
use moor_core::box_key::ControllerId;
use moor_core::seam::driver::Place;
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};

use super::{Boxes, FakeProvider, Slug};

const SLUG: &str = "moor-1a2b3c4d-417";

struct Rig {
    boxes: Arc<Boxes<FakeProvider>>,
    home: tempfile::TempDir,
}

fn rig() -> Rig {
    let home = tempfile::tempdir().unwrap();
    let boxes = Arc::new(boxes_in(home.path()));
    Rig { boxes, home }
}

fn boxes_in(home: &std::path::Path) -> Boxes<FakeProvider> {
    Boxes::new(FakeProvider::new(home.join("fake")), home)
}

fn slug() -> Slug {
    Slug::parse(SLUG).unwrap()
}

fn s(text: &str) -> String {
    text.to_owned()
}

fn open() -> BoxOp {
    BoxOp::Open {
        slug: s(SLUG),
        class: None,
        cache: None,
    }
}

fn exec(place: Place, script: &str, tty: Option<Size>) -> BoxOp {
    BoxOp::Exec {
        slug: s(SLUG),
        place,
        argv: vec![s("sh"), s("-c"), s(script)],
        env: Vec::new(),
        cwd: None,
        stdin: false,
        tty,
    }
}

impl Rig {
    /// A stream to a spawned server of `op`, with `op` already written.
    async fn ask(&self, op: &(impl serde::Serialize + Sync)) -> DuplexStream {
        let (mut client, server) = tokio::io::duplex(64 * 1024);
        let boxes = Arc::clone(&self.boxes);
        tokio::spawn(async move { boxes.serve(server).await });
        ops::write_line(&mut client, op).await.unwrap();
        client
    }

    /// Every answer to `op`, until the stream ends.
    async fn answers(&self, op: &BoxOp) -> Vec<BoxAnswer> {
        let mut client = self.ask(op).await;
        let mut said = Vec::new();
        while let Ok(a) = ops::read_line::<_, BoxAnswer>(&mut client).await {
            said.push(a);
        }
        said
    }

    /// The last answer to `op`.
    async fn one(&self, op: &BoxOp) -> BoxAnswer {
        self.answers(op).await.pop().unwrap()
    }

    /// An exec stream that has answered `streaming`.
    async fn streaming(&self, op: &BoxOp) -> DuplexStream {
        let mut client = self.ask(op).await;
        let first = ops::read_line::<_, BoxAnswer>(&mut client).await.unwrap();
        assert_eq!(first, BoxAnswer::Streaming);
        client
    }

    /// What a command wrote to stdout and stderr and how it ended.
    async fn run(&self, op: &BoxOp) -> (String, String, Exit) {
        let mut client = self.streaming(op).await;
        collect(&mut client).await
    }

    async fn opened(&self) {
        let said = self.one(&open()).await;
        assert!(matches!(said, BoxAnswer::Opened { .. }), "{said:?}");
    }
}

/// The frames of a command until its exit.
async fn collect(client: &mut DuplexStream) -> (String, String, Exit) {
    let (mut out, mut err, mut buf) = (String::new(), String::new(), Vec::new());
    loop {
        let frame = tokio::time::timeout(Duration::from_secs(10), read_frame(client, &mut buf))
            .await
            .expect("the command never ended")
            .unwrap();
        match frame {
            Some(Frame::Out(b)) => out.push_str(&String::from_utf8_lossy(b)),
            Some(Frame::Err(b)) => err.push_str(&String::from_utf8_lossy(b)),
            Some(Frame::Exit(exit)) => return (out, err, exit),
            other => panic!("unexpected {other:?}"),
        }
    }
}

/// Reads output from a terminal until it holds `needle`.
async fn until(client: &mut DuplexStream, seen: &mut String, needle: &str) {
    let mut buf = Vec::new();
    while !seen.contains(needle) {
        let frame = tokio::time::timeout(Duration::from_secs(10), read_frame(client, &mut buf))
            .await
            .unwrap_or_else(|_| panic!("no {needle:?} in {seen:?}"))
            .unwrap();
        match frame {
            Some(Frame::Out(b)) => seen.push_str(&String::from_utf8_lossy(b)),
            other => panic!("waiting for {needle:?} in {seen:?}, got {other:?}"),
        }
    }
}

#[tokio::test]
async fn open_notes_then_opens_and_a_second_open_finds_the_same_box() {
    let rig = rig();
    let said = rig.answers(&open()).await;
    assert_eq!(said.len(), 2, "{said:?}");
    assert!(matches!(&said[0], BoxAnswer::Note { .. }), "{said:?}");
    let BoxAnswer::Opened {
        workdir,
        generation,
    } = &said[1]
    else {
        panic!("{said:?}");
    };
    assert_eq!(*generation, 1);
    assert!(workdir.ends_with("workdir"), "{workdir}");
    let again = rig.one(&open()).await;
    assert_eq!(
        again,
        BoxAnswer::Opened {
            workdir: workdir.clone(),
            generation: 1
        }
    );
}

#[tokio::test]
async fn an_open_asked_again_after_its_caller_left_waits_for_it_and_finds_the_same_box() {
    let rig = rig();
    let held = rig.boxes.provider.hold_opens().await;
    let mut first = rig.ask(&open()).await;
    let said = ops::read_line::<_, BoxAnswer>(&mut first).await.unwrap();
    assert!(matches!(said, BoxAnswer::Note { .. }), "{said:?}");
    drop(first);
    let mut again = rig.ask(&open()).await;
    let early = tokio::time::timeout(
        Duration::from_millis(200),
        ops::read_line::<_, BoxAnswer>(&mut again),
    )
    .await;
    assert!(
        early.is_err(),
        "answered while the first open went on: {early:?}"
    );
    drop(held);
    let mut answers = Vec::new();
    while let Ok(a) = ops::read_line::<_, BoxAnswer>(&mut again).await {
        answers.push(a);
    }
    assert!(
        matches!(
            answers.last(),
            Some(BoxAnswer::Opened { generation: 1, .. })
        ),
        "{answers:?}"
    );
    assert_eq!(rig.boxes.provider.most_opens_at_once(), 1);
}

#[tokio::test]
async fn exec_runs_in_each_place_in_the_shared_workdir() {
    let rig = rig();
    let BoxAnswer::Opened { workdir, .. } = rig.one(&open()).await else {
        panic!("not opened");
    };
    for (place, name) in [(Place::Agent, "agent"), (Place::Work, "work")] {
        let script = "echo \"$MOOR_PLACE\"; pwd; echo oops >&2; exit 3";
        let (out, err, exit) = rig.run(&exec(place, script, None)).await;
        assert_eq!(out, format!("{name}\n{workdir}\n"));
        assert_eq!(err, "oops\n");
        assert_eq!(exit, Exit::Code(3));
    }
}

#[tokio::test]
async fn exec_honors_cwd_and_env_and_feeds_stdin_until_in_end() {
    let rig = rig();
    rig.opened().await;
    let op = BoxOp::Exec {
        slug: s(SLUG),
        place: Place::Work,
        argv: vec![s("sh"), s("-c"), s("pwd; echo \"$GREETING\"; cat")],
        env: vec![(s("GREETING"), s("it's me"))],
        cwd: Some(s("/tmp")),
        stdin: true,
        tty: None,
    };
    let mut client = rig.streaming(&op).await;
    write_frame(&mut client, &Frame::In(b"line one\n"))
        .await
        .unwrap();
    write_frame(&mut client, &Frame::In(b"line two\n"))
        .await
        .unwrap();
    write_frame(&mut client, &Frame::InEnd).await.unwrap();
    let (out, _, exit) = collect(&mut client).await;
    assert_eq!(out, "/tmp\nit's me\nline one\nline two\n");
    assert_eq!(exit, Exit::Code(0));
}

#[tokio::test]
async fn env_reaches_both_places_and_replaces_the_last_set() {
    let rig = rig();
    rig.opened().await;
    let env = |vars: Vec<(String, String)>| BoxOp::Env {
        slug: s(SLUG),
        vars,
        ca_pem: None,
    };
    let first = env(vec![(s("PROXY"), s("http://p:1")), (s("OLD"), s("x"))]);
    assert_eq!(rig.one(&first).await, BoxAnswer::Done);
    for place in [Place::Agent, Place::Work] {
        let (out, ..) = rig.run(&exec(place, "echo \"$PROXY $OLD\"", None)).await;
        assert_eq!(out, "http://p:1 x\n");
    }
    let second = env(vec![(s("PROXY"), s("http://q:2"))]);
    assert_eq!(rig.one(&second).await, BoxAnswer::Done);
    let (out, ..) = rig
        .run(&exec(Place::Work, "echo \"$PROXY [$OLD]\"", None))
        .await;
    assert_eq!(out, "http://q:2 []\n");
    let bad = env(vec![(s("A; rm -rf ~"), s("x"))]);
    assert!(matches!(rig.one(&bad).await, BoxAnswer::Refused { .. }));
}

#[tokio::test]
async fn a_terminal_has_the_size_asked_and_a_resize_reaches_the_command() {
    let rig = rig();
    rig.opened().await;
    let script = "trap 'echo \"winch $(stty size)\"; exit 0' WINCH; stty size; echo ready; \
                  while :; do sleep 0.05; done";
    let size = Size { cols: 80, rows: 24 };
    let mut client = rig.streaming(&exec(Place::Work, script, Some(size))).await;
    let mut seen = String::new();
    until(&mut client, &mut seen, "ready").await;
    assert!(seen.contains("24 80"), "{seen:?}");
    let bigger = Size {
        cols: 132,
        rows: 50,
    };
    write_frame(&mut client, &Frame::Resize(bigger))
        .await
        .unwrap();
    until(&mut client, &mut seen, "winch 50 132").await;
    let (rest, _, exit) = collect(&mut client).await;
    assert_eq!((rest.as_str(), exit), ("", Exit::Code(0)));
}

#[tokio::test]
async fn keystrokes_reach_a_terminal_and_its_output_is_all_out_frames() {
    let rig = rig();
    rig.opened().await;
    let size = Size { cols: 80, rows: 24 };
    let mut client = rig
        .streaming(&exec(
            Place::Agent,
            "read name; echo \"hi $name\"; echo oops >&2",
            Some(size),
        ))
        .await;
    write_frame(&mut client, &Frame::In(b"ada\n"))
        .await
        .unwrap();
    let (out, err, exit) = collect(&mut client).await;
    assert!(out.contains("hi ada") && out.contains("oops"), "{out:?}");
    assert_eq!((err.as_str(), exit), ("", Exit::Code(0)));
}

#[tokio::test]
async fn dropping_the_stream_ends_the_commands_process_group() {
    let rig = rig();
    rig.opened().await;
    let pids = rig.home.path().join("pid");
    let script = format!("echo $$ > {}; sleep 60 & wait", pids.display());
    let client = rig.streaming(&exec(Place::Work, &script, None)).await;
    let pid = loop {
        if let Ok(text) = std::fs::read_to_string(&pids) {
            if let Ok(pid) = text.trim().parse::<u32>() {
                break pid;
            }
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    drop(client);
    let pid = rustix::process::Pid::from_raw(i32::try_from(pid).unwrap()).unwrap();
    let alive = || rustix::process::test_kill_process(pid).is_ok();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    while alive() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the command outlived its stream"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Serves the echoes of `listener` until the test ends.
fn echo(listener: tokio::net::TcpListener) {
    tokio::spawn(async move {
        while let Ok((mut conn, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buf = [0u8; 64];
                while let Ok(n @ 1..) = conn.read(&mut buf).await {
                    let _ = conn.write_all(&buf[..n]).await;
                }
            });
        }
    });
}

#[tokio::test]
async fn forward_says_whether_a_port_listens_and_carries_its_bytes() {
    let rig = rig();
    rig.opened().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    echo(listener);
    let forward = |port, probe| BoxOp::Forward {
        slug: s(SLUG),
        port,
        probe,
    };
    assert_eq!(
        rig.one(&forward(port, true)).await,
        BoxAnswer::Listens { yes: true }
    );
    let unused = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let closed = unused.local_addr().unwrap().port();
    drop(unused);
    assert_eq!(
        rig.one(&forward(closed, true)).await,
        BoxAnswer::Listens { yes: false }
    );
    let mut conn = rig.ask(&forward(port, false)).await;
    let first = ops::read_line::<_, BoxAnswer>(&mut conn).await.unwrap();
    assert_eq!(first, BoxAnswer::Streaming);
    conn.write_all(b"ping\n").await.unwrap();
    let mut back = [0u8; 5];
    tokio::time::timeout(Duration::from_secs(10), conn.read_exact(&mut back))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&back, b"ping\n");
}

#[tokio::test]
async fn sleep_stops_the_box_and_wake_starts_it_a_generation_later() {
    let rig = rig();
    rig.opened().await;
    let slug = || s(SLUG);
    assert_eq!(
        rig.one(&BoxOp::Sleep { slug: slug() }).await,
        BoxAnswer::Done
    );
    let asleep = rig.one(&exec(Place::Work, "echo no", None)).await;
    assert_eq!(
        asleep,
        BoxAnswer::Refused {
            reason: s("the Box is asleep")
        }
    );
    assert_eq!(
        rig.one(&BoxOp::Wake { slug: slug() }).await,
        BoxAnswer::Woke { generation: 2 }
    );
    assert_eq!(
        rig.one(&BoxOp::Wake { slug: slug() }).await,
        BoxAnswer::Woke { generation: 2 }
    );
    let (out, ..) = rig.run(&exec(Place::Work, "echo yes", None)).await;
    assert_eq!(out, "yes\n");
}

#[tokio::test]
async fn close_removes_the_box_and_a_second_close_is_fine() {
    let rig = rig();
    rig.opened().await;
    let close = BoxOp::Close { slug: s(SLUG) };
    assert_eq!(rig.one(&close).await, BoxAnswer::Done);
    assert_eq!(rig.one(&close).await, BoxAnswer::Done);
    assert_eq!(
        rig.one(&exec(Place::Work, "true", None)).await,
        BoxAnswer::Gone
    );
    let list = BoxOp::List {
        controller: ControllerId::parse("1a2b3c4d").unwrap(),
    };
    assert_eq!(
        rig.one(&list).await,
        BoxAnswer::Listed { boxes: Vec::new() }
    );
    assert!(!rig.home.path().join("fake").join(SLUG).exists());
}

#[tokio::test]
async fn list_reports_this_controllers_boxes_only() {
    let rig = rig();
    rig.opened().await;
    let other = BoxOp::Open {
        slug: s("moor-0badcafe-9"),
        class: None,
        cache: None,
    };
    assert!(matches!(rig.one(&other).await, BoxAnswer::Opened { .. }));
    let list = |c: &str| BoxOp::List {
        controller: ControllerId::parse(c).unwrap(),
    };
    let BoxAnswer::Listed { boxes } = rig.one(&list("1a2b3c4d")).await else {
        panic!("not listed");
    };
    assert_eq!(boxes.len(), 1);
    assert_eq!(boxes[0].slug, SLUG);
    assert_eq!(
        (boxes[0].state.clone(), boxes[0].generation),
        (ReportState::Awake, 1)
    );
    assert_eq!(rig.boxes.reports().await.len(), 2);
}

#[tokio::test]
async fn seal_is_held_and_a_box_this_machine_does_not_have_is_gone() {
    let rig = rig();
    rig.opened().await;
    assert_eq!(
        rig.one(&BoxOp::Seal { slug: s(SLUG) }).await,
        BoxAnswer::Done
    );
    assert!(rig.boxes.provider.sealed(&slug()));
    let nobody = s("moor-1a2b3c4d-999");
    for op in [
        BoxOp::Wake {
            slug: nobody.clone(),
        },
        BoxOp::Sleep {
            slug: nobody.clone(),
        },
        BoxOp::Seal {
            slug: nobody.clone(),
        },
        BoxOp::Env {
            slug: nobody.clone(),
            vars: Vec::new(),
            ca_pem: None,
        },
    ] {
        assert_eq!(rig.one(&op).await, BoxAnswer::Gone, "{op:?}");
    }
    let bad = BoxOp::Wake {
        slug: s("../../etc"),
    };
    assert!(matches!(rig.one(&bad).await, BoxAnswer::Refused { .. }));
}

#[tokio::test]
async fn what_is_held_survives_a_restart_of_the_agent() {
    let rig = rig();
    rig.opened().await;
    assert_eq!(
        rig.one(&BoxOp::Sleep { slug: s(SLUG) }).await,
        BoxAnswer::Done
    );
    let again = Rig {
        boxes: Arc::new(boxes_in(rig.home.path())),
        home: rig.home,
    };
    let BoxAnswer::Listed { boxes } = again
        .one(&BoxOp::List {
            controller: ControllerId::parse("1a2b3c4d").unwrap(),
        })
        .await
    else {
        panic!("not listed");
    };
    assert_eq!(
        (boxes[0].state.clone(), boxes[0].generation),
        (ReportState::Asleep, 1)
    );
}

#[tokio::test]
async fn an_op_that_is_not_a_box_request_is_refused_and_runs_nothing() {
    let rig = rig();
    let marker = rig.home.path().join("ran");
    let not_box = [
        Op::Exec {
            box_id: 7,
            env: Vec::new(),
            script: format!("touch {}", marker.display()),
            timeout_secs: 5,
        },
        Op::Status,
        Op::Boxes,
    ];
    for op in &not_box {
        let mut client = rig.ask(op).await;
        let said = ops::read_line::<_, BoxAnswer>(&mut client).await.unwrap();
        let BoxAnswer::Refused { reason } = said else {
            panic!("{op:?}: {said:?}");
        };
        assert!(reason.starts_with("not a Box request"), "{reason}");
        let mut rest = Vec::new();
        client.read_to_end(&mut rest).await.unwrap();
    }
    let mut junk = rig.ask(&"not even an object").await;
    let said = ops::read_line::<_, BoxAnswer>(&mut junk).await.unwrap();
    assert!(matches!(said, BoxAnswer::Refused { .. }));
    assert!(!marker.exists());
}

#[tokio::test]
async fn an_exec_without_a_command_is_refused() {
    let rig = rig();
    rig.opened().await;
    let op = BoxOp::Exec {
        slug: s(SLUG),
        place: Place::Work,
        argv: Vec::new(),
        env: Vec::new(),
        cwd: None,
        stdin: false,
        tty: None,
    };
    assert!(matches!(rig.one(&op).await, BoxAnswer::Refused { .. }));
}

#[tokio::test]
async fn a_machine_that_provides_boxes_offers_them_with_the_protocol_it_speaks() {
    let rig = rig();
    let host = crate::Host::named("fake", rig.home.path()).unwrap();
    let offer = crate::offered(Some(&host)).unwrap();
    let boxes = offer.boxes.unwrap();
    assert_eq!(
        (boxes.protocol, boxes.provides.as_str()),
        (moor_client::link::boxes::PROTOCOL, "fake")
    );
    assert_eq!(crate::offered(None), None);
    assert!(crate::Host::named("crabbox", rig.home.path()).is_err());
}

mod down;
