//! `moor-slack mcp`, the binary the coordinator's agent starts as its tool server, spoken to
//! over its real stdin and stdout.

#![allow(clippy::indexing_slicing, reason = "test code")]

use std::io::{BufRead, BufReader, Read, Write};
use std::os::fd::OwnedFd;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use command_fds::{CommandFdExt, FdMapping};
use serde_json::{json, Value};

use super::support::controller::FakeController;
use super::support::slack_fake::{APP_TOKEN, BOT_TOKEN, TEAM};
use super::support::{Rig, BOX_NAME, CHANNEL, MCP, TS, USER};

/// A daemon whose thread [`TS`] runs Box [`BOX_NAME`], so `slack.db` holds it.
fn thread_with_a_box() -> (Rig, FakeController) {
    let (rig, controller, _bin) = Rig::start_boxes();
    rig.open_thread_with_a_box();
    (rig, controller)
}

/// The tool server for thread [`TS`] in `rig`'s state dir, acting as [`USER`], with `extra`
/// setting up the command before it spawns.
fn server(
    rig: &Rig,
    extra: impl FnOnce(&mut Command),
) -> (Child, ChildStdin, BufReader<ChildStdout>) {
    server_as(rig, &format!("slack:{TEAM}/{USER}"), extra)
}

/// Like [`server`], acting as `identity`.
fn server_as(
    rig: &Rig,
    identity: &str,
    extra: impl FnOnce(&mut Command),
) -> (Child, ChildStdin, BufReader<ChildStdout>) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_moor-slack"));
    cmd.arg("mcp")
        .arg("--state-dir")
        .arg(rig.path("state-slack"))
        .arg("--socket")
        .arg(rig.socket())
        .arg("--as")
        .arg(identity)
        .arg("--thread")
        .arg(format!("{CHANNEL}/{TS}"))
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    extra(&mut cmd);
    let mut child = cmd.spawn().unwrap();
    let stdin = child.stdin.take().unwrap();
    let stdout = BufReader::new(child.stdout.take().unwrap());
    (child, stdin, stdout)
}

/// Sends `message` as one line and, for a request, reads its answer.
fn send(
    stdin: &mut ChildStdin,
    stdout: &mut BufReader<ChildStdout>,
    message: &Value,
) -> Option<Value> {
    writeln!(stdin, "{message}").unwrap();
    stdin.flush().unwrap();
    message.get("id")?;
    let mut line = String::new();
    stdout.read_line(&mut line).unwrap();
    Some(serde_json::from_str(&line).unwrap())
}

#[test]
fn the_tool_server_answers_mcp_over_stdin_and_stdout() {
    let (rig, _controller) = thread_with_a_box();
    let (mut child, mut stdin, mut stdout) = server(&rig, |_| {});
    let init = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t", "version": "0"}}});
    let answer = send(&mut stdin, &mut stdout, &init).unwrap();
    assert_eq!(answer["id"], 1);
    assert_eq!(answer["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(answer["result"]["capabilities"], json!({"tools": {}}));
    let done = json!({"jsonrpc": "2.0", "method": "notifications/initialized"});
    assert_eq!(send(&mut stdin, &mut stdout, &done), None);
    let list = json!({"jsonrpc": "2.0", "id": "two", "method": "tools/list"});
    let answer = send(&mut stdin, &mut stdout, &list).unwrap();
    assert_eq!(answer["id"], "two", "the id is echoed as sent");
    let names: Vec<_> = answer["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        names,
        [
            "list_boxes",
            "start_box",
            "steer_box",
            "close_box",
            "answer_box"
        ]
    );
    drop(stdin);
    assert!(
        child.wait().unwrap().success(),
        "the server ends with its stdin"
    );
}

#[test]
fn the_tool_server_answers_tool_calls_from_slack_db_and_the_controller() {
    let (rig, _controller) = thread_with_a_box();
    let (_child, mut stdin, mut stdout) = server(&rig, |_| {});
    let call = json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call",
        "params": {"name": "list_boxes", "arguments": {}}});
    let answer = send(&mut stdin, &mut stdout, &call).unwrap();
    let text = answer["result"]["content"][0]["text"].as_str().unwrap();
    let want =
        format!("- Box `{BOX_NAME}` on acme/demo, asked for by <@{USER}>: lifecycle unknown");
    assert!(text.starts_with(&want), "{text}");
    let call = json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call",
        "params": {"name": "steer_box", "arguments": {"box": BOX_NAME, "text": "go on"}}});
    let answer = send(&mut stdin, &mut stdout, &call).unwrap();
    assert_eq!(answer["result"]["isError"], true);
    assert_eq!(
        answer["result"]["content"][0]["text"],
        "No turn of this thread is running."
    );
}

#[test]
fn the_tool_server_closes_a_descriptor_it_inherited_before_it_answers() {
    let (rig, _controller) = thread_with_a_box();
    let (reader, writer) = std::io::pipe().unwrap();
    let (mut child, mut stdin, mut stdout) = server(&rig, |cmd| {
        let mapping = FdMapping {
            parent_fd: OwnedFd::from(writer),
            child_fd: 5,
        };
        cmd.fd_mappings(vec![mapping]).unwrap();
    });
    let ping = json!({"jsonrpc": "2.0", "id": 1, "method": "ping"});
    assert_eq!(
        send(&mut stdin, &mut stdout, &ping).unwrap()["result"],
        json!({})
    );
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = reader;
        let mut buf = Vec::new();
        let _ = tx.send(reader.read_to_end(&mut buf).map(|_| buf));
    });
    let read = rx.recv_timeout(Duration::from_secs(5));
    let _ = child.kill();
    let _ = child.wait();
    let ended = read.expect("the inherited pipe was still open while the server answered");
    assert_eq!(ended.unwrap(), b"", "nothing reached the pipe");
}

#[test]
fn a_state_dir_without_slack_db_is_refused_and_left_as_found() {
    let rig = Rig::new();
    let (child, stdin, _stdout) = server(&rig, |_| {});
    drop(stdin);
    let out = child.wait_with_output().unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("slack.db does not exist"), "{stderr}");
    assert!(!rig.path("state-slack").exists());
}

#[test]
fn a_turn_starts_the_tool_server_named_in_its_mcp_json_and_lists_the_threads_boxes() {
    let (rig, _controller, bin) = Rig::start_boxes();
    Rig::tool_calls(&bin, None, &[("list_boxes", json!({}))]);
    rig.open_thread_with_a_box();
    super::inbox::reply(&rig, 2, USER, "what runs?");
    rig.slack.wait_updates(2);
    let results = Rig::tool_results(&bin, 1);
    let want =
        format!("- Box `{BOX_NAME}` on acme/demo, asked for by <@{USER}>: lifecycle unknown");
    assert_eq!(results.len(), 1, "{results:?}");
    assert!(results[0].0.starts_with(&want), "{results:?}");
    let args = std::fs::read_to_string(bin.join("run-0/server-args")).unwrap();
    let state = std::path::absolute(rig.path("state-slack")).unwrap();
    let want = format!(
        "mcp\n--state-dir\n{}\n--socket\n{}\n--thread\n{CHANNEL}/{TS}\n--max-open-boxes\n1\n--as\nslack:{TEAM}/{USER}\n",
        state.display(),
        rig.socket().display()
    );
    assert_eq!(
        args, want,
        "the server's argv names paths and the thread, no secret"
    );
    let env = std::fs::read_to_string(bin.join("run-0/server-env")).unwrap();
    assert!(env.lines().any(|l| l.starts_with("PATH=")), "{env}");
    let leaked: Vec<_> = [MCP, APP_TOKEN, BOT_TOKEN]
        .into_iter()
        .filter(|token| env.contains(token))
        .collect();
    assert_eq!(
        leaked,
        [] as [&str; 0],
        "the server's environment holds no token"
    );
}

#[test]
fn a_steer_in_the_requesters_turn_reaches_the_box_and_the_next_turn_reads_it_as_done() {
    let (rig, controller, bin) = Rig::start_boxes();
    controller.script(
        "steer",
        super::support::controller::Scripted::Reply(moor_core::response::Response::ok(
            "steer", None,
        )),
    );
    let steer = json!({"box": BOX_NAME, "text": "use tokio"});
    Rig::tool_calls(&bin, Some(1), &[("steer_box", steer)]);
    rig.open_thread_with_a_box();
    super::inbox::reply(&rig, 2, USER, "tell it to use tokio");
    rig.slack.wait_updates(2);
    let sent = format!("Sent to Box `{BOX_NAME}`.");
    assert_eq!(Rig::tool_results(&bin, 1), [(sent, false)]);
    let steered = controller.seen(|seen| {
        let steer = |r: &moor_core::request::Request| match r {
            moor_core::request::Request::Steer(s) => Some((s.target.to_string(), s.text.clone())),
            _ => None,
        };
        seen.requests.iter().filter_map(steer).collect::<Vec<_>>()
    });
    assert_eq!(
        steered,
        [(BOX_NAME.to_owned(), Some("use tokio".to_owned()))]
    );
    super::inbox::reply(&rig, 3, USER, "did it?");
    rig.slack.wait_updates(3);
    let stdin = std::fs::read_to_string(bin.join("run-2/stdin")).unwrap();
    let journal = format!(
        "Turn 2, <@{USER}> asked. steer_box Box `{BOX_NAME}`: \
         use tokio. Result: sent to Box `{BOX_NAME}`.\n"
    );
    assert!(stdin.contains(&journal), "{stdin}");
}

#[test]
fn a_steer_reaches_the_controller_when_the_daemon_dials_it_through_its_fd_3() {
    let (rig, controller, bin) = Rig::start_boxes_as_front();
    controller.script(
        "steer",
        super::support::controller::Scripted::Reply(moor_core::response::Response::ok(
            "steer", None,
        )),
    );
    let steer = json!({"box": BOX_NAME, "text": "use tokio"});
    Rig::tool_calls(&bin, Some(1), &[("steer_box", steer)]);
    rig.open_thread_with_a_box();
    super::inbox::reply(&rig, 2, USER, "tell it to use tokio");
    rig.slack.wait_updates(2);
    let sent = format!("Sent to Box `{BOX_NAME}`.");
    assert_eq!(Rig::tool_results(&bin, 1), [(sent, false)]);
}

#[test]
fn the_tool_server_sends_its_requests_as_the_identity_it_was_started_with() {
    let (rig, controller) = thread_with_a_box();
    let before = controller.seen(|seen| seen.requests.len());
    let who = format!("slack:{TEAM}/U0TOOLS");
    let (_child, mut stdin, mut stdout) = server_as(&rig, &who, |_| {});
    let call = json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call",
        "params": {"name": "list_boxes", "arguments": {}}});
    send(&mut stdin, &mut stdout, &call).unwrap();
    let (sent, acting) = controller.seen(|seen| {
        (
            seen.requests[before..].to_vec(),
            seen.acting[before..].to_vec(),
        )
    });
    assert!(
        sent.iter()
            .any(|r| matches!(r, moor_core::request::Request::Show(_))),
        "{sent:?}"
    );
    assert_eq!(acting, vec![Some(who); sent.len()]);
}

#[test]
fn the_tool_server_refuses_an_identity_that_is_not_one() {
    let rig = Rig::new();
    let (child, stdin, _stdout) = server_as(&rig, "no-namespace", |_| {});
    drop(stdin);
    let out = child.wait_with_output().unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("--as"), "{stderr}");
}
