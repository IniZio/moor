//! Tests for the tool server's JSON-RPC edges: tool refusals as results the model reads,
//! errors for unknown methods and bad lines, and the protocol version it answers with. The
//! daemon tests speak to the binary for the rest.
#![allow(clippy::indexing_slicing, reason = "test code")]

use serde_json::{json, Value};

use super::serve;
use crate::coordinator::tools::{Call, Reply};

/// The answers to `lines`, with every tool call answered by `answer`.
fn exchange(lines: &[Value], answer: &mut dyn FnMut(&Call) -> Reply) -> Vec<Value> {
    let input = lines
        .iter()
        .fold(String::new(), |all, l| all + &l.to_string() + "\n");
    let mut output = Vec::new();
    serve(&mut input.as_bytes(), &mut output, answer).unwrap();
    String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn request(id: i64, method: &str, params: &Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
}

#[test]
fn a_bad_call_is_a_result_with_is_error_and_runs_nothing() {
    let mut ran = false;
    let mut answer = |_: &Call| {
        ran = true;
        Reply {
            text: String::new(),
            is_error: false,
        }
    };
    let call = request(7, "tools/call", &json!({"name": "rm_rf", "arguments": {}}));
    let answers = exchange(&[call], &mut answer);
    assert_eq!(answers[0]["result"]["isError"], true);
    assert_eq!(
        answers[0]["result"]["content"][0]["text"],
        "There is no tool `rm_rf`."
    );
    assert!(!ran);
}

#[test]
fn an_unknown_method_and_a_line_that_is_not_json_get_errors() {
    let mut answer = |_: &Call| unreachable!("no tool runs");
    let input = format!("{}\nnot json\n", request(4, "resources/list", &json!({})));
    let mut output = Vec::new();
    serve(&mut input.as_bytes(), &mut output, &mut answer).unwrap();
    let answers: Vec<Value> = String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(answers[0]["id"], 4);
    assert_eq!(answers[0]["error"]["code"], -32601);
    assert_eq!(answers[1]["id"], Value::Null);
    assert_eq!(answers[1]["error"]["code"], -32700);
}

#[test]
fn an_unknown_protocol_version_gets_the_newest_one_spoken() {
    let mut answer = |_: &Call| unreachable!("no tool runs");
    let answers = exchange(
        &[request(
            1,
            "initialize",
            &json!({"protocolVersion": "1999-01-01"}),
        )],
        &mut answer,
    );
    assert_eq!(answers[0]["result"]["protocolVersion"], "2025-06-18");
}
