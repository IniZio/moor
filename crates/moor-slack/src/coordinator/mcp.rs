//! `moor-slack mcp`, the coordinator's tool server. The agent starts it for one turn and
//! talks to it in MCP's JSON-RPC over stdin and stdout, one message per line. It reads the
//! thread's rows in `slack.db` and calls the controller socket. It reads no Slack token and
//! no agent credential, and before it reads anything it points every descriptor it inherited
//! above 2 at `/dev/null`.

use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, BufRead, Read as _, Write};
use std::os::fd::OwnedFd;
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use command_fds::{CommandFdExt as _, FdMapping};
use moor_client::{Client, Timeouts};
use serde_json::{json, Value};

use super::tools::{self, Call, Reply, Scope, SPECS};
use crate::db::Db;

/// The newest MCP version this server speaks.
const NEWEST: &str = "2025-06-18";

/// Every MCP version this server speaks.
const VERSIONS: [&str; 3] = ["2024-11-05", "2025-03-26", NEWEST];

/// Longest a tool waits on the controller before it says the controller is not answering.
const CONTROLLER_WAIT: Duration = Duration::from_secs(30);

/// Longest accepted message line.
const MAX_LINE: u64 = 1 << 20;

/// What `moor-slack mcp` needs from its command line.
#[derive(Debug, Clone)]
pub struct ToolSettings {
    /// The front's state dir, which holds `slack.db`.
    pub state_dir: PathBuf,
    /// Controller socket to dial.
    pub socket: PathBuf,
    /// The thread, as `<channel>/<thread ts>`.
    pub thread: String,
}

/// Makes sure no descriptor above 2 that the process inherited still reaches what it was
/// opened on.
///
/// When one does, the server runs `argv` again with `--inherited-closed` added and each such
/// descriptor pointed at its own `/dev/null`. With that flag already given, any descriptor
/// above 2 that is not `/dev/null` is an error.
///
/// # Errors
/// Says which descriptor stayed open, or why the server could not run again.
pub fn close_inherited(argv: &[OsString], already: bool) -> Result<(), String> {
    let open = inherited().map_err(|e| format!("list /proc/self/fd: {e}"))?;
    let Some(first) = open.first() else {
        return Ok(());
    };
    if already {
        return Err(format!("inherited descriptor {first} could not be closed"));
    }
    let Some((program, rest)) = argv.split_first() else {
        return Err("no program to run again".to_owned());
    };
    let mappings = open
        .iter()
        .map(|fd| {
            let null = File::open("/dev/null")?;
            let child_fd = i32::try_from(*fd).map_err(io::Error::other)?;
            Ok(FdMapping {
                parent_fd: OwnedFd::from(null),
                child_fd,
            })
        })
        .collect::<io::Result<Vec<_>>>()
        .map_err(|e| format!("open /dev/null: {e}"))?;
    let program = std::env::current_exe().map_or_else(|_| program.clone(), OsString::from);
    let mut again = Command::new(&program);
    again.args(rest).arg("--inherited-closed");
    again
        .fd_mappings(mappings)
        .map_err(|e| format!("map descriptors: {e}"))?;
    let err = again.exec();
    Err(format!("run {}: {err}", program.to_string_lossy()))
}

/// Open descriptors above 2 that are not `/dev/null`. The descriptor that reads `/proc/self/fd`
/// is closed by the time each is checked, so it is not counted.
fn inherited() -> io::Result<Vec<u32>> {
    let names: Vec<OsString> = fs::read_dir("/proc/self/fd")?
        .map(|entry| entry.map(|e| e.file_name()))
        .collect::<io::Result<_>>()?;
    let mut open: Vec<u32> = names
        .iter()
        .filter_map(|n| n.to_str()?.parse().ok())
        .filter(|fd| *fd > 2)
        .filter(|fd| {
            fs::read_link(format!("/proc/self/fd/{fd}"))
                .is_ok_and(|target| target != Path::new("/dev/null"))
        })
        .collect();
    open.sort_unstable();
    Ok(open)
}

/// Serves the coordinator's tools for one thread on stdin and stdout until stdin ends.
///
/// # Errors
/// Says why the server could not start: a thread that is not `<channel>/<ts>`, a state dir
/// with no `slack.db`, or a `slack.db` at another schema. Also an error on stdin or stdout.
pub fn serve_tools(settings: &ToolSettings) -> Result<(), String> {
    let (channel, thread_ts) = settings
        .thread
        .split_once('/')
        .filter(|(c, t)| !c.is_empty() && !t.is_empty())
        .ok_or_else(|| format!("--thread {} is not <channel>/<ts>", settings.thread))?;
    let path = settings.state_dir.join("slack.db");
    if !path.is_file() {
        return Err(format!("{} does not exist", path.display()));
    }
    let db = Db::open(&path).map_err(|e| format!("open slack.db: {e}"))?;
    let client = Client::new(&settings.socket).with_timeouts(Timeouts {
        read: CONTROLLER_WAIT,
        write: Duration::from_secs(10),
    });
    let mut call = |request| client.request(request, |_| {});
    let mut scope = Scope {
        db: &db,
        channel,
        thread_ts,
        call: &mut call,
    };
    let mut run = |call: &Call| tools::run(&mut scope, call);
    serve(&mut io::stdin().lock(), &mut io::stdout().lock(), &mut run)
        .map_err(|e| format!("mcp: {e}"))
}

/// Answers each message on `input` on `output` until `input` ends; `run` runs a tool call.
pub(crate) fn serve(
    input: &mut impl BufRead,
    output: &mut dyn Write,
    run: &mut dyn FnMut(&Call) -> Reply,
) -> io::Result<()> {
    let mut line = String::new();
    loop {
        line.clear();
        let read = input.by_ref().take(MAX_LINE).read_line(&mut line)?;
        if read == 0 {
            return Ok(());
        }
        if !line.ends_with('\n') && u64::try_from(read).is_ok_and(|n| n >= MAX_LINE) {
            return Err(io::Error::other("a message is longer than 1 MiB"));
        }
        if line.trim().is_empty() {
            continue;
        }
        if let Some(answer) = answer(line.trim_end(), run) {
            serde_json::to_writer(&mut *output, &answer)?;
            output.write_all(b"\n")?;
            output.flush()?;
        }
    }
}

/// The answer to one message; `None` for a notification.
fn answer(line: &str, run: &mut dyn FnMut(&Call) -> Reply) -> Option<Value> {
    let Ok(message) = serde_json::from_str::<Value>(line) else {
        return Some(error(&Value::Null, -32700, "not JSON"));
    };
    let id = message.get("id").cloned();
    let Some(method) = message.get("method").and_then(Value::as_str) else {
        return Some(error(&id.unwrap_or(Value::Null), -32600, "no method"));
    };
    let id = id?;
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    let result = match method {
        "initialize" => initialize(&params),
        "ping" => json!({}),
        "tools/list" => tools_list(),
        "tools/call" => tools_call(&params, run),
        other => return Some(error(&id, -32601, &format!("no method {other}"))),
    };
    Some(json!({"jsonrpc": "2.0", "id": id, "result": result}))
}

/// The `initialize` result: the client's version when this server speaks it, else the
/// newest one it speaks.
fn initialize(params: &Value) -> Value {
    let asked = params.get("protocolVersion").and_then(Value::as_str);
    let version = asked.filter(|v| VERSIONS.contains(v)).unwrap_or(NEWEST);
    json!({
        "protocolVersion": version,
        "capabilities": {"tools": {}},
        "serverInfo": {"name": "moor", "version": env!("CARGO_PKG_VERSION")},
    })
}

/// The `tools/list` result.
fn tools_list() -> Value {
    let tools: Vec<Value> = SPECS
        .iter()
        .map(|s| json!({"name": s.name, "description": s.description, "inputSchema": (s.input)()}))
        .collect();
    json!({ "tools": tools })
}

/// The `tools/call` result. A refusal is a result with `isError`, so the model reads it.
fn tools_call(params: &Value, run: &mut dyn FnMut(&Call) -> Reply) -> Value {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let args = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let reply = Call::parse(name, &args).map_or_else(
        |why| Reply {
            text: why,
            is_error: true,
        },
        |call| run(&call),
    );
    json!({
        "content": [{"type": "text", "text": reply.text}],
        "isError": reply.is_error,
    })
}

/// A JSON-RPC error answer.
fn error(id: &Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

#[cfg(test)]
mod tests;
