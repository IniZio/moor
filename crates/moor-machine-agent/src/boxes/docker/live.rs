//! The Docker provider against a real daemon, driven through `Boxes::serve` over in-memory links.
//!
//! Ignored by default. Run it with the images to use, built from `images/`:
//!
//! ```sh
//! MOOR_TEST_AGENT_IMAGE=... MOOR_TEST_WORK_IMAGE=... \
//!   cargo nextest run -p moor-machine-agent --run-ignored only docker::live
//! ```
//!
//! It touches only the objects of its two test Boxes, named from [`MAIN`] and [`OUTLIVING`], and
//! the network [`NETWORK`], and removes each by exact name at the start and the end, panic or not.

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
use moor_client::link::ops;
use moor_core::seam::driver::Place;
use tokio::io::{AsyncReadExt, DuplexStream};

use super::names::Names;
use super::{Docker, Images};
use crate::boxes::{Boxes, Slug};

const MAIN: &str = "moor-3b3b3b3b-9001";
const OUTLIVING: &str = "moor-3b3b3b3b-9002";
const NETWORK: &str = "boxsmoke3b-net";
const WORKDIR: &str = "/home/agent/moor";
const LISTEN_PORT: u16 = 18080;

type Rig = Arc<Boxes<Docker>>;

mod cache;

/// Removes the test Boxes' containers and volumes, and the network when nothing is on it, each by
/// exact name.
fn sweep() {
    for text in [MAIN, OUTLIVING] {
        let names = Names::of(&Slug::parse(text).unwrap());
        for container in [&names.work, &names.agent] {
            docker(&["rm", "-f", container]);
        }
        for volume in names.volumes() {
            docker(&["volume", "rm", "-f", volume]);
        }
    }
    docker(&["network", "rm", NETWORK]);
}

fn docker(args: &[&str]) -> bool {
    std::process::Command::new("docker")
        .args(args)
        .output()
        .is_ok_and(|out| out.status.success())
}

/// Sweeps when the test ends, however it ends.
struct Cleanup;

impl Drop for Cleanup {
    fn drop(&mut self) {
        sweep();
    }
}

fn image(var: &str) -> String {
    std::env::var(var).unwrap_or_else(|_| panic!("set {var} to an image built from images/"))
}

fn rig(home: &std::path::Path) -> Rig {
    let images = Images {
        agent: Some(image("MOOR_TEST_AGENT_IMAGE")),
        work: Some(image("MOOR_TEST_WORK_IMAGE")),
        ..Images::default()
    };
    Arc::new(Boxes::new(Docker::new(images, Some(NETWORK)), home))
}

/// Says which step the test is at, so a hang shows where.
#[allow(clippy::print_stderr, reason = "progress of a live run")]
fn step(what: &str) {
    eprintln!("live: {what}");
}

fn s(text: &str) -> String {
    text.to_owned()
}

async fn ask(rig: &Rig, op: &BoxOp) -> DuplexStream {
    let (mut client, server) = tokio::io::duplex(64 * 1024);
    let rig = Arc::clone(rig);
    tokio::spawn(async move { rig.serve(server).await });
    ops::write_line(&mut client, op).await.unwrap();
    client
}

async fn answers(rig: &Rig, op: &BoxOp) -> Vec<BoxAnswer> {
    let mut client = ask(rig, op).await;
    let mut said = Vec::new();
    while let Ok(a) = ops::read_line::<_, BoxAnswer>(&mut client).await {
        said.push(a);
    }
    said
}

async fn one(rig: &Rig, op: &BoxOp) -> BoxAnswer {
    answers(rig, op).await.pop().unwrap()
}

fn open(slug: &str) -> BoxOp {
    BoxOp::Open {
        slug: s(slug),
        class: Some(s("small")),
        cache: None,
    }
}

fn exec(slug: &str, place: Place, script: &str, tty: Option<Size>, stdin: bool) -> BoxOp {
    BoxOp::Exec {
        slug: s(slug),
        place,
        argv: vec![s("bash"), s("-c"), s(script)],
        env: Vec::new(),
        cwd: None,
        stdin,
        tty,
    }
}

async fn streaming(rig: &Rig, op: &BoxOp) -> DuplexStream {
    let mut client = ask(rig, op).await;
    let first = ops::read_line::<_, BoxAnswer>(&mut client).await.unwrap();
    assert_eq!(first, BoxAnswer::Streaming);
    client
}

/// What a command wrote to stdout and how it ended; a stderr line fails the message, not the run.
async fn collect(client: &mut DuplexStream) -> (String, Exit) {
    let (mut out, mut err, mut buf) = (String::new(), String::new(), Vec::new());
    loop {
        let frame = tokio::time::timeout(Duration::from_secs(60), read_frame(client, &mut buf))
            .await
            .expect("the command never ended")
            .unwrap();
        match frame {
            Some(Frame::Out(b)) => out.push_str(&String::from_utf8_lossy(b)),
            Some(Frame::Err(b)) => err.push_str(&String::from_utf8_lossy(b)),
            Some(Frame::Exit(exit)) => {
                if !err.is_empty() {
                    out = format!("{out}[stderr: {err}]");
                }
                return (out, exit);
            }
            other => panic!("unexpected {other:?}"),
        }
    }
}

/// [`collect`] of a command just started.
async fn run(rig: &Rig, op: &BoxOp) -> (String, Exit) {
    collect(&mut streaming(rig, op).await).await
}

/// Reads terminal output until it holds `needle`.
async fn until(client: &mut DuplexStream, seen: &mut String, needle: &str) {
    let mut buf = Vec::new();
    while !seen.contains(needle) {
        let frame = tokio::time::timeout(Duration::from_secs(20), read_frame(client, &mut buf))
            .await
            .unwrap_or_else(|_| panic!("no {needle:?} in {seen:?}"))
            .unwrap();
        match frame {
            Some(Frame::Out(b)) => seen.push_str(&String::from_utf8_lossy(b)),
            other => panic!("waiting for {needle:?} in {seen:?}, got {other:?}"),
        }
    }
}

async fn state_of(rig: &Rig, slug: &str) -> ReportState {
    let controller = Slug::parse(slug).unwrap().controller().unwrap();
    let BoxAnswer::Listed { boxes } = one(rig, &BoxOp::List { controller }).await else {
        panic!("not listed");
    };
    boxes
        .into_iter()
        .find(|b| b.slug == slug)
        .unwrap_or_else(|| panic!("{slug} is not listed"))
        .state
}

#[tokio::test]
#[ignore = "needs a Docker daemon and the Box images"]
#[allow(
    clippy::too_many_lines,
    reason = "one Box life reads best as one script"
)]
async fn a_docker_box_lives_from_open_to_close() {
    sweep();
    let _cleanup = Cleanup;
    let home = tempfile::tempdir().unwrap();
    let rig = rig(home.path());
    let names = Names::of(&Slug::parse(MAIN).unwrap());

    step("open");
    // Open: notes, then the workdir and generation 1; a second open finds the same Box.
    let said = answers(&rig, &open(MAIN)).await;
    let opened = said.last().unwrap();
    assert_eq!(
        *opened,
        BoxAnswer::Opened {
            workdir: s(WORKDIR),
            generation: 1
        },
        "{said:?}"
    );
    assert_eq!(one(&rig, &open(MAIN)).await, *opened);
    assert_eq!(state_of(&rig, MAIN).await, ReportState::Awake);

    step("env");
    // Env reaches both places' login shells.
    let env = BoxOp::Env {
        slug: s(MAIN),
        vars: vec![(s("BOXLIVE"), s("it's set"))],
        ca_pem: None,
    };
    assert_eq!(one(&rig, &env).await, BoxAnswer::Done);
    let script = "echo \"env=$BOXLIVE\"; pwd";
    for place in [Place::Agent, Place::Work] {
        let (out, exit) = run(&rig, &exec(MAIN, place, script, None, false)).await;
        assert_eq!(out, format!("env=it's set\n{WORKDIR}\n"), "{place:?}");
        assert_eq!(exit, Exit::Code(0));
    }

    step("shared tree and stdin");
    // The two places share the work tree, and the work place carries stdin and an exit code.
    run(
        &rig,
        &exec(MAIN, Place::Agent, "echo shared > marker", None, false),
    )
    .await;
    let (out, _) = run(
        &rig,
        &exec(MAIN, Place::Work, "cat marker; id -u", None, false),
    )
    .await;
    assert_eq!(out, "shared\n1000\n");
    let mut cat = streaming(&rig, &exec(MAIN, Place::Work, "cat; exit 7", None, true)).await;
    write_frame(&mut cat, &Frame::In(b"piped in\n"))
        .await
        .unwrap();
    write_frame(&mut cat, &Frame::InEnd).await.unwrap();
    let (out, exit) = collect(&mut cat).await;
    assert_eq!((out.as_str(), exit), ("piped in\n", Exit::Code(7)));

    step("terminal and resize");
    // A terminal has the size asked, and a resize reaches the command.
    let size = Size { cols: 80, rows: 24 };
    let watch = "stty size; trap 'echo winch $(stty size)' WINCH; \
                 while :; do read -t 0.2 -r l && [ \"$l\" = q ] && exit 0; done";
    let mut term = streaming(&rig, &exec(MAIN, Place::Agent, watch, Some(size), true)).await;
    let mut seen = String::new();
    until(&mut term, &mut seen, "24 80").await;
    let bigger = Size {
        cols: 132,
        rows: 50,
    };
    write_frame(&mut term, &Frame::Resize(bigger))
        .await
        .unwrap();
    until(&mut term, &mut seen, "winch 50 132").await;
    write_frame(&mut term, &Frame::In(b"q\n")).await.unwrap();

    step("forward");
    // Forward: a listener in the Box's network, probed and then carried.
    let listen = format!(
        "nohup setsid socat TCP-LISTEN:{LISTEN_PORT},bind=127.0.0.1,reuseaddr,fork \
         SYSTEM:'echo hello-from-box' >/dev/null 2>&1 &"
    );
    run(&rig, &exec(MAIN, Place::Agent, &listen, None, false)).await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    let probe = |port| BoxOp::Forward {
        slug: s(MAIN),
        port,
        probe: true,
    };
    assert_eq!(
        one(&rig, &probe(LISTEN_PORT)).await,
        BoxAnswer::Listens { yes: true }
    );
    assert_eq!(
        one(&rig, &probe(LISTEN_PORT + 1)).await,
        BoxAnswer::Listens { yes: false }
    );
    let carry = BoxOp::Forward {
        slug: s(MAIN),
        port: LISTEN_PORT,
        probe: false,
    };
    let mut conn = streaming(&rig, &carry).await;
    let mut got = String::new();
    conn.read_to_string(&mut got).await.unwrap();
    assert_eq!(got, "hello-from-box\n");

    step("sleep and wake");
    // Sleep stops both places and refuses commands; wake starts the agent place first, so the
    // work place joins its new network namespace and answers again, with its environment.
    assert_eq!(
        one(&rig, &BoxOp::Sleep { slug: s(MAIN) }).await,
        BoxAnswer::Done
    );
    assert_eq!(state_of(&rig, MAIN).await, ReportState::Asleep);
    for place in [&names.work, &names.agent] {
        assert!(!docker(&["exec", place, "true"]), "{place} still runs");
    }
    let refused = one(&rig, &exec(MAIN, Place::Work, "true", None, false)).await;
    assert!(matches!(refused, BoxAnswer::Refused { .. }), "{refused:?}");
    let woke = one(&rig, &BoxOp::Wake { slug: s(MAIN) }).await;
    assert_eq!(woke, BoxAnswer::Woke { generation: 2 });
    let (out, _) = run(
        &rig,
        &exec(
            MAIN,
            Place::Work,
            "echo \"env=$BOXLIVE\"; cat marker",
            None,
            false,
        ),
    )
    .await;
    assert_eq!(out, "env=it's set\nshared\n");
    assert_eq!(state_of(&rig, MAIN).await, ReportState::Awake);

    step("seal");
    // Seal is not supported yet, and says so.
    let sealed = one(&rig, &BoxOp::Seal { slug: s(MAIN) }).await;
    assert!(matches!(sealed, BoxAnswer::Refused { .. }), "{sealed:?}");

    step("close");
    // Close removes both places and every volume; closing again is fine.
    assert_eq!(
        one(&rig, &BoxOp::Close { slug: s(MAIN) }).await,
        BoxAnswer::Done
    );
    assert_eq!(
        one(&rig, &BoxOp::Close { slug: s(MAIN) }).await,
        BoxAnswer::Done
    );
    for container in [&names.work, &names.agent] {
        assert!(
            !docker(&["inspect", container]),
            "{container} is still there"
        );
    }
    for volume in names.volumes() {
        assert!(
            !docker(&["volume", "inspect", volume]),
            "{volume} is still there"
        );
    }
    assert_eq!(
        one(&rig, &BoxOp::Sleep { slug: s(MAIN) }).await,
        BoxAnswer::Gone
    );

    step("open outlives the caller");
    // An open outlives a controller that hangs up at once: the Box still gets made and kept.
    drop(ask(&rig, &open(OUTLIVING)).await);
    let mut kept = false;
    for _ in 0..240 {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let controller = Slug::parse(OUTLIVING).unwrap().controller().unwrap();
        if let BoxAnswer::Listed { boxes } = one(&rig, &BoxOp::List { controller }).await {
            if boxes
                .iter()
                .any(|b| b.slug == OUTLIVING && b.state == ReportState::Awake)
            {
                kept = true;
                break;
            }
        }
    }
    assert!(kept, "the dropped open never finished");
    assert_eq!(
        one(&rig, &BoxOp::Close { slug: s(OUTLIVING) }).await,
        BoxAnswer::Done
    );
}
