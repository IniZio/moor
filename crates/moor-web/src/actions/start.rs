//! Starting work from the browser: a new request, or one more worker on a request.
//!
//! A request is a root host Session without a Box, bound to the front `web` under the
//! request key. Workers are Sessions started as its children.

use std::collections::hash_map::RandomState;
use std::fs::DirBuilder;
use std::hash::{BuildHasher, Hasher};
use std::os::unix::fs::DirBuilderExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use moor_core::request::{
    BindSessionReq, EndHostSessionReq, OpenHostSessionReq, Request, StartReq,
};
use moor_core::response::ExitClass;
use moor_core::seam::forge::ForgeRepo;
use moor_core::{BoxName, SessionId};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::db::{Db, RequestRow};
use crate::http::{Handler, HttpRequest, HttpResponse, Reply};
use crate::jobs::{JobFailure, JobHandle, Jobs};
use moor_client::Client;

#[cfg(test)]
mod tests;

/// Everything the start routes need.
#[derive(Clone)]
pub struct StartCtx {
    /// Controller connection.
    pub client: Client,
    /// The front's own database.
    pub db: Arc<Mutex<Db>>,
    /// Background jobs the routes spawn into.
    pub jobs: Jobs,
    /// Directory of the front's own state; request workdirs live below it.
    pub state_dir: PathBuf,
    /// Current time in unix seconds.
    pub clock: fn() -> i64,
}

/// JSON body of `POST /requests`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewBody {
    /// Repo to work in.
    repo: String,
    /// Name of the first worker.
    name: String,
    /// Prompt text; empty means none.
    prompt: String,
}

/// JSON body of `POST /requests/{key}/workers`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkerBody {
    /// Name of the worker.
    name: String,
    /// Prompt text; empty means none.
    prompt: String,
}

/// Serves the start routes on POST and hands everything else to `fallback`.
pub fn handler(ctx: StartCtx, fallback: Handler) -> Handler {
    Arc::new(move |req: &HttpRequest| -> Reply {
        if req.method != "POST" {
            return fallback(req);
        }
        if req.path == "/requests" {
            return create(&ctx, &req.body).into();
        }
        let key = req
            .path
            .strip_prefix("/requests/")
            .and_then(|rest| rest.strip_suffix("/workers"));
        key.map_or_else(
            || fallback(req),
            |key| add_worker(&ctx, key, &req.body).into(),
        )
    })
}

/// Opens a request and starts its first worker in the background.
fn create(ctx: &StartCtx, body: &[u8]) -> HttpResponse {
    let b: NewBody = match parse(body) {
        Ok(b) => b,
        Err(msg) => return bad(msg),
    };
    let box_name = match name(&b.name) {
        Ok(box_name) => box_name,
        Err(msg) => return bad(msg),
    };
    if let Err(e) = ForgeRepo::parse(&b.repo) {
        return bad(e.to_string());
    }
    let key = new_key();
    let (ctx2, key2) = (ctx.clone(), key.clone());
    let prompt = prompt(b.prompt);
    let job = ctx.jobs.spawn("new request", move |h| {
        let root = open_root(&ctx2, h, &key2, &b.repo)?;
        let start = start(&ctx2.client, h, &b.repo, root, box_name, prompt)?;
        Ok(json!({"key": key2, "root": root, "start": start}))
    });
    accepted(&key, &job)
}

/// Starts one more worker under an open request.
fn add_worker(ctx: &StartCtx, key: &str, body: &[u8]) -> HttpResponse {
    let b: WorkerBody = match parse(body) {
        Ok(b) => b,
        Err(msg) => return bad(msg),
    };
    let box_name = match name(&b.name) {
        Ok(box_name) => box_name,
        Err(msg) => return bad(msg),
    };
    let row = ctx
        .db
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .request(key);
    let row = match row {
        Ok(Some(row)) => row,
        Ok(None) => return HttpResponse::text("404 Not Found", "unknown request"),
        Err(e) => return HttpResponse::text("500 Internal Server Error", e.to_string()),
    };
    if row.ended_at.is_some() {
        return HttpResponse::text("409 Conflict", "request has ended");
    }
    let (client, prompt) = (ctx.client.clone(), prompt(b.prompt));
    let job = ctx.jobs.spawn("add worker", move |h| {
        let start = start(&client, h, &row.repo, row.root_session, box_name, prompt)?;
        Ok(json!({"key": row.key, "root": row.root_session, "start": start}))
    });
    accepted(key, &job)
}

/// Creates the workdir, opens and binds the root Session and
/// records the request. A root opened but not recorded is ended again.
fn open_root(
    ctx: &StartCtx,
    h: &JobHandle,
    key: &str,
    repo: &str,
) -> Result<SessionId, JobFailure> {
    h.progress("creating the request workdir");
    let dir = ctx.state_dir.join("requests").join(key);
    let mut builder = DirBuilder::new();
    builder.recursive(true).mode(0o700);
    builder
        .create(&dir)
        .map_err(|e| fail("workdir", format!("create {}: {e}", dir.display())))?;
    let workdir = std::path::absolute(&dir)
        .ok()
        .and_then(|p| p.to_str().map(str::to_owned))
        .ok_or_else(|| fail("workdir", "workdir path is not absolute utf-8"))?;
    let opened = call(
        &ctx.client,
        h,
        ("open-host-session", "opening the request session"),
        Request::OpenHostSession(OpenHostSessionReq {
            workdir: Some(workdir),
            agent: None,
            agent_session_id: None,
            parent: None,
            key: None,
        }),
    )?;
    let root = opened
        .get("session")
        .and_then(Value::as_i64)
        .map(SessionId::new)
        .ok_or_else(|| fail("open-host-session", "reply carries no session id"))?;
    bind_and_save(ctx, h, key, repo, root).inspect_err(|_| {
        h.progress("ending the unbound request session");
        let end = Request::EndHostSession(EndHostSessionReq { session: root });
        let _ = ctx.client.request(end, |note| h.progress(note));
    })?;
    Ok(root)
}

/// Binds `root` to the request key and records the request.
fn bind_and_save(
    ctx: &StartCtx,
    h: &JobHandle,
    key: &str,
    repo: &str,
    root: SessionId,
) -> Result<(), JobFailure> {
    call(
        &ctx.client,
        h,
        ("bind-session", "binding the request"),
        Request::BindSession(BindSessionReq {
            front: "web".into(),
            key: key.to_owned(),
            session: root,
            owner: "web".into(),
        }),
    )?;
    h.progress("saving the request");
    let row = RequestRow {
        key: key.to_owned(),
        root_session: root,
        repo: repo.to_owned(),
        created_at: (ctx.clock)(),
        ended_at: None,
    };
    ctx.db
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert_request(&row)
        .map_err(|e| fail("save-request", e.to_string()))?;
    Ok(())
}

/// Starts a worker Session under `root`, forwarding the controller's progress.
fn start(
    client: &Client,
    h: &JobHandle,
    repo: &str,
    root: SessionId,
    box_name: BoxName,
    prompt: Option<String>,
) -> Result<Value, JobFailure> {
    let repo = ForgeRepo::parse(repo).map_err(|e| fail("repo", e.to_string()))?;
    call(
        client,
        h,
        ("start", "starting the worker"),
        Request::Start(StartReq {
            name: box_name,
            prompt,
            issue: None,
            base: None,
            model: None,
            provider: None,
            no_agent: false,
            resume: false,
            repo: Some(repo),
            parent: Some(root),
            identity: None,
        }),
    )
}

/// Makes one socket call for `step` and returns the envelope's value.
fn call(
    client: &Client,
    h: &JobHandle,
    (step, line): (&str, &str),
    request: Request,
) -> Result<Value, JobFailure> {
    h.progress(line);
    let envelope = client
        .request(request, |note| h.progress(note))
        .map_err(|e| fail(step, e.to_string()))?;
    if envelope.class != ExitClass::Ok {
        let message = envelope
            .error
            .map_or_else(|| format!("{step} failed"), |e| e.message);
        return Err(fail(step, message));
    }
    Ok(envelope.value.unwrap_or(Value::Null))
}

/// A failure of `step`.
fn fail(step: &str, message: impl Into<String>) -> JobFailure {
    JobFailure {
        step: step.to_owned(),
        message: message.into(),
    }
}

/// An unguessable request key: 32 hex digits from per-process random hashing.
fn new_key() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let part = |salt: u64| {
        let mut hasher = RandomState::new().build_hasher();
        hasher.write_u64(n);
        hasher.write_u64(salt);
        hasher.finish()
    };
    format!("{:016x}{:016x}", part(0), part(1))
}

/// The 202 reply naming the request and the job doing the work.
fn accepted(key: &str, job: &str) -> HttpResponse {
    HttpResponse {
        status: "202 Accepted",
        content_type: "application/json".into(),
        headers: Vec::new(),
        body: json!({"key": key, "job": job}).to_string().into_bytes(),
    }
}

/// A 400 reply.
fn bad(msg: String) -> HttpResponse {
    HttpResponse::text("400 Bad Request", msg)
}

/// Prompt text, with an empty one meaning none.
fn prompt(text: String) -> Option<String> {
    Some(text).filter(|g| !g.is_empty())
}

/// Parses a JSON body.
fn parse<'a, T: Deserialize<'a>>(body: &'a [u8]) -> Result<T, String> {
    serde_json::from_slice(body).map_err(|e| format!("bad json: {e}"))
}

/// Parses a Box name.
fn name(s: &str) -> Result<BoxName, String> {
    BoxName::parse_allow_bare(s).map_err(|e| e.to_string())
}
