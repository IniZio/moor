//! The Box-facing listener against a fake controller. The tests set `X-Moor-Box` themselves, as
//! the Proxy does, and check that the report reaches the controller with that Box, that the
//! controller's reply reaches the agent as it is, and that nothing else is served.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::os::unix::net::UnixListener;
use std::sync::mpsc;

use moor_core::response::{ErrorKind, ErrorView, Response};
use serde_json::{json, Value};

/// The replies of the table in the spec, each keyed by the step id the test reports, with
/// whether the reply is an error: what the fake controller answers for that step.
const ROWS: [(&str, &str, bool); 8] = [
    (
        "norun",
        "This Box has no running workflow run, so there is no step to report.",
        true,
    ),
    (
        "elsewhere",
        "Step elsewhere is not the current step. The current step is audit.",
        true,
    ),
    ("refused", "Refused: no file matches `a.md`.", true),
    (
        "nextwrk",
        "That step is done. Do only the next step, then call report_step.",
        false,
    ),
    (
        "nextcheck",
        "End your turn. moor runs the check and sends you the result.",
        false,
    ),
    (
        "nextapprove",
        "End your turn. moor asks the owner and sends you the answer.",
        false,
    ),
    ("delivered", "Done.", false),
    (
        "again",
        "Already recorded. Do only the next step, then call report_step.",
        false,
    ),
];

struct Listener {
    addr: String,
    seen: mpsc::Receiver<Value>,
    _dir: tempfile::TempDir,
    /// The service key the requests carry, as the Proxy would stamp it.
    key: Option<String>,
}

/// A Box listener whose controller answers each `step-report` from [`ROWS`], or refuses a step
/// called `refuse`.
fn listener(controller_up: bool) -> Listener {
    listener_keyed(controller_up, None)
}

/// As [`listener`], requiring `key` in `X-Moor-Service-Key` when it is given.
fn listener_keyed(controller_up: bool, key: Option<&str>) -> Listener {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("authgear.sock");
    let (tx, seen) = mpsc::channel();
    if controller_up {
        let controller = UnixListener::bind(&socket).unwrap();
        std::thread::spawn(move || {
            for stream in controller.incoming().flatten() {
                let tx = tx.clone();
                std::thread::spawn(move || {
                    let mut reply = stream.try_clone().unwrap();
                    for line in BufReader::new(stream).lines().map_while(Result::ok) {
                        let frame: Value = serde_json::from_str(&line).unwrap();
                        if frame["frame"] == "hello" {
                            writeln!(reply, "{line}").unwrap();
                            continue;
                        }
                        if frame["frame"] != "request" {
                            continue;
                        }
                        tx.send(frame["request"].clone()).unwrap();
                        let envelope = if frame["request"]["cmd"] == "box-tool" {
                            box_tool(&frame, &mut reply)
                        } else if frame["request"]["cmd"] == "output-report" {
                            output(&frame["request"])
                        } else if frame["request"]["cmd"] == "reading-hand-in" {
                            hand_in(&frame["request"])
                        } else {
                            let step = frame["request"]["report"]["step"].as_str().unwrap_or("");
                            answer(step)
                        };
                        let out =
                            json!({"frame": "response", "id": frame["id"], "envelope": envelope});
                        writeln!(reply, "{out}").unwrap();
                    }
                });
            }
        });
    }
    let mut boxes = moor_gate::Boxes::new(moor_client::Client::new(&socket)).unwrap();
    if let Some(key) = key {
        boxes = boxes.requiring_key(key.to_owned());
    }
    let tcp = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = tcp.local_addr().unwrap().to_string();
    std::thread::spawn(move || boxes.serve(tcp));
    Listener {
        addr,
        seen,
        _dir: dir,
        key: key.map(str::to_owned),
    }
}

/// The fake controller's answer to a `box-tool` `frame`: from Box 9, a work Box, the refusal;
/// an exec of `sleep` is still running as run 7, and a wait on run 7 sees it end.
fn box_tool(frame: &Value, _out: &mut dyn Write) -> Response {
    let req = &frame["request"];
    if req["from"] == 9 {
        let view = ErrorView {
            kind: ErrorKind::Refused,
            message: "exec_in_box, start_box and close_box serve only an agent place that a \
                      task has claimed, and this is a work Box."
                .to_owned(),
            hints: vec![],
            context: vec![],
            detail: None,
        };
        return Response::err("box-tool", view);
    }
    if req["call"]["tool"] == "exec" && req["call"]["command"] == "sleep" {
        return Response::ok(
            "box-tool",
            Some(json!({"run": 7, "state": "running", "stdout": "started\n",
                "stderr": "", "stdout_truncated": false, "stderr_truncated": false})),
        );
    }
    if req["call"]["tool"] == "exec" {
        return Response::ok(
            "box-tool",
            Some(json!({"run": 6, "state": "exited", "exit": 0,
                "stdout": "hi\n", "stderr": "", "stdout_truncated": false,
                "stderr_truncated": false})),
        );
    }
    if req["call"]["tool"] == "wait" {
        return Response::ok(
            "box-tool",
            Some(
                json!({"run": req["call"]["run"], "state": "exited", "exit": 0,
                "stdout": "done\n", "stderr": "",
                "stdout_truncated": false, "stderr_truncated": false}),
            ),
        );
    }
    if req["call"]["tool"] == "close" {
        return Response::ok(
            "close",
            Some(json!({"box": {"name": req["call"]["target"]}})),
        );
    }
    Response::ok("start", Some(json!({"name": req["call"]["name"]})))
}

/// The fake controller's answer to an `output-report`: a title of `full` is refused, as a Box
/// at its limit is.
fn output(req: &Value) -> Response {
    if req["title"] == "full" {
        let view = ErrorView {
            kind: ErrorKind::Refused,
            message: "this Box already has 100 outputs".to_owned(),
            hints: vec![],
            context: vec![],
            detail: None,
        };
        return Response::err("output-report", view);
    }
    Response::ok(
        "output-report",
        Some(
            json!({"reply": format!("kept {} {}", req["kind"].as_str().unwrap(), req["url"].as_str().unwrap())}),
        ),
    )
}

/// The fake controller's answer to a `reading-hand-in`: a first quote of `nowhere` is refused.
fn hand_in(req: &Value) -> Response {
    if req["reading"]["phrases"][0]["quote"]["exact"] == "nowhere" {
        let view = ErrorView {
            kind: ErrorKind::Refused,
            message: "quote `nowhere` is not in the prompt; copy the prompt's own words".to_owned(),
            hints: vec![],
            context: vec![],
            detail: None,
        };
        return Response::err("reading-hand-in", view);
    }
    Response::ok("reading-hand-in", Some(json!({"reply": "accepted"})))
}

fn answer(step: &str) -> Response {
    if step == "refuse" {
        let view = ErrorView {
            kind: ErrorKind::Refused,
            message: "step-report serves only a front whose controller config sets \
                      `step_reports: true`"
                .to_owned(),
            hints: vec![],
            context: vec![],
            detail: None,
        };
        return Response::err("step-report", view);
    }
    let (reply, error) = ROWS
        .iter()
        .find(|(s, ..)| *s == step)
        .map_or(("unknown step", true), |(_, reply, error)| (*reply, *error));
    Response::ok("step-report", Some(json!({"reply": reply, "error": error})))
}

impl Listener {
    /// The JSON-RPC reply to `method`, after a handshake, with `box_header` as the Box the
    /// Proxy named, or none at all.
    fn rpc(&self, box_header: Option<&str>, method: &str, params: Value) -> Value {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .build()
            .into();
        let send = |body: &Value, session: Option<&str>| {
            let mut req = agent
                .post(format!("http://{}/mcp", self.addr))
                .header("Content-Type", "application/json")
                .header("Accept", "application/json, text/event-stream")
                .header("Host", "moor.moor.internal");
            if let Some(id) = box_header {
                req = req.header("X-Moor-Box", id);
            }
            if let Some(key) = &self.key {
                req = req.header("X-Moor-Service-Key", key);
            }
            if let Some(session) = session {
                req = req.header("Mcp-Session-Id", session);
            }
            let mut res = req.send(body.to_string()).unwrap();
            let session = res
                .headers()
                .get("mcp-session-id")
                .map(|v| v.to_str().unwrap().to_owned());
            let text = res.body_mut().read_to_string().unwrap();
            (
                serde_json::from_str::<Value>(&text).unwrap_or(Value::Null),
                session,
            )
        };
        let init = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-06-18", "capabilities": {},
            "clientInfo": {"name": "test", "version": "0"}}});
        let (opened, session) = send(&init, None);
        assert!(opened.get("result").is_some(), "initialize: {opened}");
        send(
            &json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
            session.as_deref(),
        );
        send(
            &json!({"jsonrpc": "2.0", "id": 2, "method": method, "params": params}),
            session.as_deref(),
        )
        .0
    }

    /// `report_step` for `step` as Box `box_header`.
    fn report(&self, box_header: Option<&str>, step: &str) -> Value {
        let reply = self.rpc(
            box_header,
            "tools/call",
            json!({"name": "report_step", "arguments": {
                "step": step, "result": "ok", "evidence": ["a.md"], "note": "n"}}),
        );
        reply["result"].clone()
    }
}

impl Listener {
    /// `report_output` of `title` as Box `box_header`.
    fn report_output(&self, box_header: Option<&str>, title: &str) -> Value {
        let reply = self.rpc(
            box_header,
            "tools/call",
            json!({"name": "report_output", "arguments": {
                "kind": "github.comment", "url": "https://example.com/acme/pull/1#c1",
                "title": title}}),
        );
        reply["result"].clone()
    }
}

fn text(result: &Value) -> &str {
    result["content"][0]["text"].as_str().unwrap_or_default()
}

#[test]
fn only_the_moor_server_with_report_step_and_the_place_tools_is_served() {
    let l = listener(true);
    let reply = l.rpc(Some("7"), "tools/list", json!({}));
    let tools = reply["result"]["tools"].as_array().unwrap();
    let mut names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "close_box",
            "exec_in_box",
            "exec_wait",
            "hand_in_reading",
            "report_output",
            "report_step",
            "start_box"
        ],
        "{tools:?}"
    );
    let report = tools.iter().find(|t| t["name"] == "report_step").unwrap();
    let mut args: Vec<&str> = report["inputSchema"]["properties"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    args.sort_unstable();
    assert_eq!(args, ["evidence", "note", "result", "step"]);
    let resources = l.rpc(Some("7"), "resources/list", json!({}));
    assert_eq!(resources["result"]["resources"], json!([]), "{resources}");
    let person_tool = l.rpc(
        Some("7"),
        "tools/call",
        json!({"name": "list_boxes", "arguments": {}}),
    );
    assert!(person_tool.get("error").is_some(), "{person_tool}");
    assert!(l.seen.try_recv().is_err());
}

#[test]
fn every_reply_of_the_table_reaches_the_agent_as_the_controller_said_it() {
    let l = listener(true);
    for (step, reply, error) in ROWS {
        let result = l.report(Some("7"), step);
        assert_eq!(text(&result), reply, "{step}");
        assert_eq!(
            result["isError"].as_bool().unwrap_or(false),
            error,
            "{step}"
        );
    }
}

#[test]
fn the_report_reaches_the_controller_for_the_box_the_proxy_named_and_no_other() {
    let l = listener(true);
    l.report(Some("41"), "delivered");
    l.report(Some("42"), "delivered");
    let first = l.seen.recv().unwrap();
    assert_eq!(
        first,
        json!({"cmd": "step-report", "box_id": 41, "report": {
            "step": "delivered", "result": "ok", "evidence": ["a.md"], "note": "n"}})
    );
    assert_eq!(l.seen.recv().unwrap()["box_id"], 42);
}

#[test]
fn an_output_reaches_the_controller_for_the_box_the_proxy_named() {
    let l = listener(true);
    let result = l.report_output(Some("41"), "Evidence comment");
    assert_eq!(
        text(&result),
        "kept github.comment https://example.com/acme/pull/1#c1",
        "{result}"
    );
    assert_eq!(
        l.seen.recv().unwrap(),
        json!({"cmd": "output-report", "box_id": 41, "kind": "github.comment",
            "url": "https://example.com/acme/pull/1#c1", "title": "Evidence comment"})
    );
}

#[test]
fn an_output_without_a_box_header_or_refused_by_moor_reaches_the_agent_as_an_error() {
    let l = listener(true);
    let unnamed = l.report_output(None, "Evidence comment");
    assert_eq!(unnamed["isError"], true, "{unnamed}");
    assert!(l.seen.try_recv().is_err());
    let full = l.report_output(Some("7"), "full");
    assert_eq!(full["isError"], true, "{full}");
    assert_eq!(text(&full), "this Box already has 100 outputs", "{full}");
    let away = listener(false).report_output(Some("7"), "Evidence comment");
    assert!(text(&away).contains("call report_output again"), "{away}");
}

#[test]
fn a_hand_in_reaches_the_controller_for_the_box_the_proxy_named_and_a_refusal_reaches_the_agent() {
    let l = listener(true);
    let call = |exact: &str| {
        let doc = json!({"phrases": [{"quote": {"exact": exact}, "mark": {"kind": "asked"}}],
            "effects": []});
        l.rpc(
            Some("41"),
            "tools/call",
            json!({"name": "hand_in_reading", "arguments": doc}),
        )["result"]
            .clone()
    };
    let accepted = call("Never merge");
    assert_eq!(text(&accepted), "accepted", "{accepted}");
    let seen = l.seen.recv().unwrap();
    assert_eq!(seen["cmd"], "reading-hand-in");
    assert_eq!(seen["box_id"], 41);
    assert_eq!(
        seen["reading"]["phrases"][0]["quote"]["exact"],
        "Never merge"
    );
    let refused = call("nowhere");
    assert_eq!(refused["isError"], true, "{refused}");
    assert!(text(&refused).contains("`nowhere`"), "{refused}");
}

#[test]
fn a_request_without_a_valid_box_header_reaches_no_controller() {
    let l = listener(true);
    for header in [None, Some("0"), Some("-3"), Some("seven"), Some("")] {
        let result = l.report(header, "delivered");
        assert_eq!(result["isError"], true, "{header:?}: {result}");
        assert!(
            text(&result).contains("does not say which Box"),
            "{header:?}: {result}"
        );
    }
    assert!(l.seen.try_recv().is_err());
}

#[test]
fn a_controller_refusal_reaches_the_agent_as_an_error() {
    let l = listener(true);
    let result = l.report(Some("7"), "refuse");
    assert_eq!(result["isError"], true, "{result}");
    assert!(text(&result).contains("step_reports: true"), "{result}");
}

#[test]
fn a_controller_that_is_away_tells_the_agent_nothing_is_recorded() {
    let l = listener(false);
    let result = l.report(Some("7"), "delivered");
    assert_eq!(result["isError"], true, "{result}");
    assert!(text(&result).contains("Nothing is recorded"), "{result}");
}

#[test]
fn a_step_id_that_is_no_id_is_refused_before_the_controller() {
    let l = listener(true);
    let result = l.report(Some("7"), "Not An Id");
    assert_eq!(result["isError"], true, "{result}");
    assert!(l.seen.try_recv().is_err());
}

/// The status and body of a bare POST to `/mcp` carrying `key` as the service key, if any.
fn post_with_key(addr: &str, key: Option<&str>) -> (u16, String) {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .build()
        .into();
    let mut req = agent
        .post(format!("http://{addr}/mcp"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("X-Moor-Box", "7");
    if let Some(key) = key {
        req = req.header("X-Moor-Service-Key", key);
    }
    let init = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
        "protocolVersion": "2025-06-18", "capabilities": {},
        "clientInfo": {"name": "test", "version": "0"}}});
    let mut res = req.send(init.to_string()).unwrap();
    let status = res.status().as_u16();
    (status, res.body_mut().read_to_string().unwrap())
}

#[test]
fn a_matching_service_key_is_served() {
    let l = listener_keyed(true, Some("s3cret"));
    let result = l.report(Some("7"), "delivered");
    assert_eq!(text(&result), "Done.", "{result}");
    assert_eq!(l.seen.recv().unwrap()["box_id"], 7);
}

#[test]
fn a_missing_or_wrong_service_key_is_refused_alike() {
    let l = listener_keyed(true, Some("s3cret"));
    let missing = post_with_key(&l.addr, None);
    let wrong = post_with_key(&l.addr, Some("s3cret2"));
    let short = post_with_key(&l.addr, Some("s3"));
    assert_eq!(missing.0, 401, "{missing:?}");
    assert_eq!(missing, wrong);
    assert_eq!(missing, short);
    assert!(l.seen.try_recv().is_err());
    assert_eq!(post_with_key(&l.addr, Some("s3cret")).0, 200);
}

#[test]
fn with_no_service_key_required_a_request_without_one_is_served() {
    let l = listener(true);
    assert_eq!(post_with_key(&l.addr, None).0, 200);
    assert_eq!(post_with_key(&l.addr, Some("anything")).0, 200);
}

#[test]
fn an_empty_service_key_env_stops_the_gate_from_starting() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_moor-gate"))
        .args(["--listen", "127.0.0.1:0", "--issuer", "https://i.example"])
        .args(["--client-id", "c", "--front-socket", "/nonexistent.sock"])
        .env("MOOR_SERVICE_KEY", "")
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("MOOR_SERVICE_KEY is set but empty"),
        "{stderr}"
    );
}

/// `moor-gate --listen off` with a service key and a Box listener on a free loopback port, and
/// no sign-in flags.
fn box_only_gate(key: &str) -> (moor_testkit::ChildGuard, String) {
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let addr = format!("127.0.0.1:{port}");
    let child = std::process::Command::new(env!("CARGO_BIN_EXE_moor-gate"))
        .args(["--listen", "off", "--box-listen", &addr])
        .args(["--front-socket", "/nonexistent.sock"])
        .env("MOOR_SERVICE_KEY", key)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    (moor_testkit::ChildGuard::new(child), addr)
}

#[test]
fn a_gate_with_its_public_listener_off_serves_boxes_without_any_sign_in_flag() {
    let (mut gate, addr) = box_only_gate("s3cret");
    let listening = (0..100).any(|_| {
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert!(gate.try_wait().unwrap().is_none(), "the gate exited early");
        std::net::TcpStream::connect(&addr).is_ok()
    });
    assert!(listening, "the Box listener never came up");
    assert_eq!(post_with_key(&addr, None).0, 401);
    assert_eq!(post_with_key(&addr, Some("s3cret")).0, 200);
}

#[test]
fn a_gate_with_a_public_listener_still_needs_its_sign_in_flags() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_moor-gate"))
        .args([
            "--listen",
            "127.0.0.1:0",
            "--front-socket",
            "/nonexistent.sock",
        ])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("--issuer"), "{stderr}");
}

mod place;
