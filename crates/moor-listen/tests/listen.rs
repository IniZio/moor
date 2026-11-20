//! The `moor-listen` binary against the real gate and a fake controller that serves watches
//! and Box reads from state the test edits.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::disallowed_methods,
    clippy::print_stderr,
    clippy::option_if_let_else,
    clippy::significant_drop_tightening,
    clippy::needless_pass_by_value,
    clippy::missing_docs_in_private_items,
    reason = "tests"
)]

mod support;

use std::process::Command;
use std::sync::mpsc;
use std::time::Duration;

use moor_testkit::oidc::now;
use moor_testkit::ChildGuard;
use serde_json::json;
use support::{world, Listen, World, CLIENT_ID};

#[test]
fn idled_prints_the_line_with_the_first_line_of_agent_said() {
    let world = world(&["repo/a"]);
    let listen = world.listen();
    world.wait_live(1);
    world.happen("repo/a", 1, "idled", "Opened the PR.\nSecond line.");
    assert_eq!(listen.line(), "moor: box repo/a idled: Opened the PR.");
}

#[test]
fn a_stall_prints_how_long_and_what_the_agent_was_doing() {
    let world = world(&["repo/a"]);
    let listen = world.listen();
    world.wait_live(1);
    let payload = json!({"doing": "Bash: cargo test", "quiet_secs": 725});
    world.happen_with("repo/a", 1, "agent_stalled", payload);
    assert_eq!(
        listen.line(),
        "moor: box repo/a stalled: no progress for 12 min while Bash: cargo test"
    );
}

#[test]
fn a_low_node_disk_prints_the_node_and_its_free_space() {
    let world = world(&["repo/a"]);
    let listen = world.listen();
    world.wait_live(1);
    let payload = json!({"node": "hv", "free_mib": 15_360, "total_mib": 102_400});
    world.happen_with("repo/a", 1, "node_disk_low", payload);
    assert_eq!(
        listen.line(),
        "moor: box repo/a node_disk_low: hv has 15 GiB free of 100 GiB"
    );
}

#[test]
fn a_lost_disk_prints_what_is_gone() {
    let world = world(&["repo/a"]);
    let listen = world.listen();
    world.wait_live(1);
    world.happen_with("repo/a", 1, "disk_lost", json!({}));
    assert_eq!(
        listen.line(),
        "moor: box repo/a disk_lost: its machine was replaced, so anything not pushed is gone"
    );
}

#[test]
fn new_hands_print_whether_the_conversation_came_back() {
    let world = world(&["repo/a"]);
    let listen = world.listen();
    world.wait_live(1);
    world.happen_with("repo/a", 1, "session_restored", json!({"archive_at": "t"}));
    assert_eq!(
        listen.line(),
        "moor: box repo/a session_restored: its conversation up to its last saved turn is back; \
         a prompt sent after that was not sent again"
    );
    world.happen_with(
        "repo/a",
        2,
        "session_lost",
        json!({"reason": "no archive was saved"}),
    );
    assert_eq!(
        listen.line(),
        "moor: box repo/a session_lost: its conversation could not be brought back, so its \
         agent started without it; a prompt it was working on was not sent again"
    );
}

#[test]
fn a_resume_prints_how_long_the_stall_lasted() {
    let world = world(&["repo/a"]);
    let listen = world.listen();
    world.wait_live(1);
    world.happen_with("repo/a", 1, "agent_resumed", json!({"quiet_secs": 1500}));
    assert_eq!(listen.line(), "moor: box repo/a resumed after 25 min");
}

#[test]
fn busy_prints_nothing() {
    let world = world(&["repo/a", "repo/b"]);
    let listen = world.listen();
    world.wait_live(1);
    world.happen("repo/a", 1, "busy", "working");
    world.happen("repo/b", 2, "needs_input", "");
    assert_eq!(listen.line(), "moor: box repo/b needs_input");
}

#[test]
fn an_idle_that_follows_a_question_does_not_repeat_the_last_reply() {
    let world = world(&["repo/a"]);
    let listen = world.listen();
    world.wait_live(1);
    world.happen("repo/a", 1, "idled", "ready");
    assert_eq!(listen.line(), "moor: box repo/a idled: ready");
    let asked = json!({"question": "Which color?", "options": ["red", "blue"]});
    world.happen_with("repo/a", 2, "needs_input", asked);
    world
        .fake
        .lock()
        .unwrap()
        .said
        .insert("repo/a".to_owned(), "ready".to_owned());
    assert_eq!(
        listen.line(),
        "moor: box repo/a needs_input: Which color? (red, blue)"
    );
    world.happen("repo/a", 3, "idled", "ready");
    std::thread::sleep(Duration::from_millis(500));
    world.happen("repo/a", 4, "idled", "Picked blue.");
    assert_eq!(listen.line(), "moor: box repo/a idled: Picked blue.");
}

#[test]
fn a_box_started_after_open_is_picked_up_through_list_changed() {
    let world = world(&["repo/a"]);
    let listen = world.listen();
    world.wait_live(1);
    world.fake.lock().unwrap().boxes.push("repo/c".to_owned());
    world.emit(1, Some("repo/c"), "started");
    world.wait_live(2);
    world.happen("repo/c", 2, "idled", "Done.");
    assert_eq!(listen.line(), "moor: box repo/c idled: Done.");
}

#[test]
fn a_dropped_stream_prints_lost_then_reconnected() {
    let world = world(&["repo/a"]);
    let listen = world.listen();
    world.wait_live(1);
    world.drop_watches();
    let lost = listen.line();
    assert!(
        lost.starts_with("moor-listen: stream lost (") && lost.ends_with("); reconnecting"),
        "{lost}"
    );
    world.come_back();
    assert_eq!(listen.line(), "moor-listen: reconnected");
    world.wait_live(1);
    world.happen("repo/a", 1, "idled", "Back.");
    assert_eq!(listen.line(), "moor: box repo/a idled: Back.");
}

#[test]
fn a_rejected_token_prints_the_token_line() {
    let world = world(&["repo/a"]);
    std::fs::write(world.dir.path().join("bad"), "").unwrap();
    let listen = world.listen();
    assert_eq!(
        listen.line(),
        "moor-listen: token rejected; call any moor tool so the client refreshes it"
    );
}

#[test]
fn the_token_command_reruns_on_reconnect() {
    let world = world(&["repo/a"]);
    let listen = world.listen();
    world.wait_live(1);
    let before = world.runs();
    assert!(before >= 1);
    world.drop_watches();
    listen.line();
    world.come_back();
    assert_eq!(listen.line(), "moor-listen: reconnected");
    assert!(world.runs() > before, "{} runs", world.runs());
}

/// Sets what the issuer's refreshed tokens say, so the gate takes them.
fn issuer_signs_for_gate(world: &World) {
    let url = format!("http://{}/mcp", world.gate);
    *world.issuer.id_claims.lock().unwrap() = json!({"aud": url, "sub": "alice-sub",
        "scope": "moor:read", "email": "Alice@Example.com", "email_verified": true});
}

/// A sign-in to this world's gate holding `access`, which expires at `expires_at`.
fn signed_in(world: &World, access: &str, expires_at: i64) -> moor_client::session::Store {
    let store = moor_client::session::Store::new(world.dir.path().join("state"));
    store
        .write(&moor_client::session::Credentials {
            server: format!("http://{}", world.gate),
            issuer: world.issuer.base.clone(),
            client_id: CLIENT_ID.to_owned(),
            token_endpoint: format!("{}/token", world.issuer.base),
            refresh_token: "refresh-0".to_owned(),
            access_token: access.to_owned(),
            expires_at,
            email: None,
        })
        .unwrap();
    store
}

/// `moor listen` as the library runs it, with the lines it says.
fn listen_with_session(world: &World, store: moor_client::session::Store) -> Listen {
    let listen = moor_listen::Listen {
        url: format!("http://{}/mcp", world.gate),
        bearer: Box::new(moor_listen::Session::new(
            store,
            &format!("http://{}", world.gate),
        )),
        idle: Duration::from_secs(45),
        signed_out: Duration::from_millis(100),
        only: Vec::new(),
        workflows: Vec::new(),
    };
    let (tx, lines) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = listen.run(&mut |line| {
            tx.send(line.to_owned())
                .map_err(|_| std::io::ErrorKind::BrokenPipe.into())
        });
    });
    Listen {
        child: ChildGuard::new(Command::new("true").spawn().unwrap()),
        lines,
    }
}

#[test]
fn an_expired_session_refreshes_and_connects() {
    let world = world(&["repo/a"]);
    issuer_signs_for_gate(&world);
    let listen = listen_with_session(&world, signed_in(&world, "stale", 0));
    world.wait_live(1);
    world.happen("repo/a", 1, "idled", "Fresh.");
    assert_eq!(listen.line(), "moor: box repo/a idled: Fresh.");
    assert_eq!(
        world
            .issuer
            .refreshes
            .load(std::sync::atomic::Ordering::SeqCst),
        1
    );
}

#[test]
fn a_token_the_gate_refuses_is_refreshed_once() {
    let world = world(&["repo/a"]);
    issuer_signs_for_gate(&world);
    let far = i64::try_from(now()).unwrap() + 3000;
    let listen = listen_with_session(&world, signed_in(&world, "revoked", far));
    world.wait_live(1);
    world.happen("repo/a", 1, "idled", "Renewed.");
    assert_eq!(listen.line(), "moor: box repo/a idled: Renewed.");
    assert_eq!(
        world
            .issuer
            .refreshes
            .load(std::sync::atomic::Ordering::SeqCst),
        1
    );
}

#[test]
fn not_signed_in_says_so_once_and_connects_after_a_login() {
    let world = world(&["repo/a"]);
    issuer_signs_for_gate(&world);
    let state = world.dir.path().join("state");
    let origin = format!("http://{}", world.gate);
    let listen = listen_with_session(&world, moor_client::session::Store::new(&state));
    assert_eq!(
        listen.line(),
        format!("moor-listen: not signed in to {origin}; run moor login --server {origin}")
    );
    std::thread::sleep(Duration::from_millis(400));
    signed_in(&world, "stale", 0);
    assert_eq!(listen.line(), "moor-listen: reconnected");
    world.wait_live(1);
    world.happen("repo/a", 1, "idled", "Welcome.");
    assert_eq!(listen.line(), "moor: box repo/a idled: Welcome.");
}

#[test]
fn box_flag_prints_only_the_named_boxes() {
    let world = world(&["repo/a", "repo/b"]);
    let listen = world.listen_only(&["repo/b"]);
    world.wait_live(1);
    world.happen("repo/a", 1, "needs_input", "");
    world.happen("repo/b", 2, "idled", "Mine.");
    assert_eq!(listen.line(), "moor: box repo/b idled: Mine.");
}

#[test]
fn box_flag_ignores_a_box_started_after_open() {
    let world = world(&["repo/a"]);
    let listen = world.listen_only(&["repo/a"]);
    world.wait_live(1);
    world.fake.lock().unwrap().boxes.push("repo/c".to_owned());
    world.emit(1, Some("repo/c"), "started");
    world.wait_live(2);
    world.happen("repo/c", 2, "idled", "Not yours.");
    world.happen("repo/a", 3, "idled", "Yours.");
    assert_eq!(listen.line(), "moor: box repo/a idled: Yours.");
}

#[test]
fn box_flag_names_a_box_the_server_does_not_list() {
    let world = world(&["repo/a"]);
    let listen = world.listen_only(&["repo/nope"]);
    assert_eq!(
        listen.line(),
        "moor-listen: no Box named repo/nope is visible; waiting for it"
    );
}

#[test]
fn box_flag_exits_once_every_named_box_has_closed() {
    let world = world(&["repo/a", "repo/b", "repo/c"]);
    let mut listen = world.listen_only(&["repo/a", "repo/b"]);
    world.wait_live(1);
    world.happen("repo/a", 1, "closed", "");
    assert_eq!(listen.line(), "moor: box repo/a closed");
    assert!(listen.child.try_wait().unwrap().is_none(), "exited early");
    world.happen("repo/c", 2, "closed", "");
    world.happen("repo/b", 3, "closed", "");
    assert_eq!(listen.line(), "moor: box repo/b closed");
    for _ in 0..500 {
        if let Some(status) = listen.child.try_wait().unwrap() {
            assert!(status.success());
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("moor-listen kept running after every named Box closed");
}
