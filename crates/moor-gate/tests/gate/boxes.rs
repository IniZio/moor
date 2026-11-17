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
}

/// A Box listener whose controller answers each `step-report` from [`ROWS`], or refuses a step
/// called `refuse`.
fn listener(controller_up: bool) -> Listener {
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
                        tx.send(frame["request"].clone()).unwrap();
                        let step = frame["request"]["report"]["step"].as_str().unwrap_or("");
                        let envelope = answer(step);
                        let out =
                            json!({"frame": "response", "id": frame["id"], "envelope": envelope});
                        writeln!(reply, "{out}").unwrap();
                    }
                });
            }
        });
    }
    let boxes = moor_gate::Boxes::new(moor_client::Client::new(&socket)).unwrap();
    let tcp = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = tcp.local_addr().unwrap().to_string();
    std::thread::spawn(move || boxes.serve(tcp));
    Listener {
        addr,
        seen,
        _dir: dir,
    }
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

fn text(result: &Value) -> &str {
    result["content"][0]["text"].as_str().unwrap_or_default()
}

#[test]
fn only_the_moor_server_with_report_step_is_served() {
    let l = listener(true);
    let reply = l.rpc(Some("7"), "tools/list", json!({}));
    let tools = reply["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 1, "{tools:?}");
    assert_eq!(tools[0]["name"], "report_step");
    let mut args: Vec<&str> = tools[0]["inputSchema"]["properties"]
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
