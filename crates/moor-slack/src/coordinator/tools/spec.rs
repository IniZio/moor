//! What each coordinator tool does and takes, as the agent's tool list shows it.

use serde_json::{json, Value};

/// What a tool does, for the agent's tool list.
pub(crate) struct Spec {
    /// The tool's name.
    pub(crate) name: &'static str,
    /// What it does.
    pub(crate) description: &'static str,
    /// Its parameters as a JSON schema, each with a one-line description.
    pub(crate) input: fn() -> Value,
}

/// Every tool the coordinator gets.
pub(crate) const SPECS: [Spec; 5] = [
    Spec {
        name: "list_boxes",
        description: "List this thread's Boxes: name, repo, requester and state as the \
                      controller reports it now, and each open prompt with its id.",
        input: || json!({"type": "object", "properties": {}, "additionalProperties": false}),
    },
    Spec {
        name: "start_box",
        description: "Start a Box, a remote machine whose coding agent works on one goal in one \
                      repo. Works only when the thread's starter wrote in this turn, and only \
                      while the thread has fewer open Boxes than its limit.",
        input: || {
            json!({
                "type": "object",
                "properties": {
                    "goal": {"type": "string", "description": "What the Box's agent should do, in plain words."},
                    "repo": {"type": "string", "description": "One of the channel's repos as owner/name; leave it out for the channel's default."}
                },
                "required": ["goal"],
                "additionalProperties": false
            })
        },
    },
    Spec {
        name: "steer_box",
        description: "Send text to the coding agent of one of this thread's Boxes. Works only \
                      when the person who asked for the Box wrote in this turn.",
        input: || {
            json!({
                "type": "object",
                "properties": {
                    "box": {"type": "string", "description": "The Box's name, as list_boxes shows it."},
                    "text": {"type": "string", "description": "What to tell the Box's agent."}
                },
                "required": ["box", "text"],
                "additionalProperties": false
            })
        },
    },
    Spec {
        name: "close_box",
        description: "Close one of this thread's Boxes. The thread is told when it has closed, \
                      or why the controller refused. Works only when the person who asked for \
                      the Box wrote in this turn.",
        input: || {
            json!({
                "type": "object",
                "properties": {
                    "box": {"type": "string", "description": "The Box's name, as list_boxes shows it."}
                },
                "required": ["box"],
                "additionalProperties": false
            })
        },
    },
    Spec {
        name: "answer_box",
        description: "Pick an option of a question one of this thread's Boxes asked, by the \
                      prompt's id from list_boxes. Works only while the prompt is open, and only \
                      when the person who asked for the Box wrote in this turn.",
        input: || {
            json!({
                "type": "object",
                "properties": {
                    "box": {"type": "string", "description": "The Box's name, as list_boxes shows it."},
                    "prompt": {"type": "integer", "description": "The open prompt's id, as list_boxes shows it."},
                    "choice": {"type": "integer", "description": "The number of the option to pick, from 1."}
                },
                "required": ["box", "prompt", "choice"],
                "additionalProperties": false
            })
        },
    },
];

/// The names of every tool the coordinator gets.
pub(crate) fn names() -> Vec<&'static str> {
    SPECS.iter().map(|s| s.name).collect()
}
