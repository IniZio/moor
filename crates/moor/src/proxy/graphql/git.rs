//! The git host's requests: a push meets the push guard, everything else passes untouched.

use std::io::{self, Write};

use super::body::Body;
use super::http::{request_framing, write_refusal, Head};
use super::route::canonical_path;
use super::serve::{bad, forward, pass, request_head, Client, Flow, Payload, Up};
use super::{check::Relay, GitSpec, Shared};
use crate::proxy::push::{judge, read_commands, report, Commands, PushRefusal, Refused};

/// The last path segment of a git push.
const RECEIVE_PACK: &str = "/git-receive-pack";

/// Reads and handles one request to the git host.
pub(super) fn serve_one(shared: &Shared, client: &mut Client, up: &mut Up<'_>) -> Flow {
    let Some(git) = shared.git.as_ref() else {
        return Flow::Close;
    };
    let head = match request_head(client) {
        Ok(h) => h,
        Err(flow) => return flow,
    };
    let path = canonical_path(&head.target);
    match path.strip_suffix(RECEIVE_PACK) {
        Some(repo) => {
            // The canonical path is lowercased; a plainly spelled target keeps the repo's case.
            let raw = head.target.split(['?', '#']).next().unwrap_or_default();
            let repo = raw
                .get(..repo.len())
                .filter(|r| r.eq_ignore_ascii_case(repo))
                .unwrap_or(repo);
            push(shared, git, client, up, (&head, repo))
        }
        None => pass(shared, client, up, &head),
    }
}

/// Reads the ref updates at the head of a push to `repo` and forwards the push only when the Box
/// owns every ref it moves.
fn push(
    shared: &Shared,
    git: &GitSpec,
    client: &mut Client,
    up: &mut Up<'_>,
    (head, repo): (&Head, &str),
) -> Flow {
    let Some(mut body) = request_framing(head)
        .ok()
        .and_then(|f| Body::new(client, &f))
    else {
        return bad(client, 400, "moor: malformed request");
    };
    let encoded = head
        .header("content-encoding")
        .is_some_and(|c| !c.eq_ignore_ascii_case("identity"));
    let commands = if encoded {
        Err(Refused::Unreadable("moor cannot read a compressed push"))
    } else {
        read_commands(&mut body)
    };
    let verdict = commands
        .as_ref()
        .map_err(Clone::clone)
        .and_then(|c| judge(&c.updates, &git.owned));
    if let Err(refused) = verdict {
        // The answer waits for the whole push, so git reads it instead of a broken pipe.
        let drained = io::copy(&mut body, &mut io::sink()).is_ok();
        return refuse(
            shared,
            git,
            client,
            (repo, commands.ok().as_ref()),
            &refused,
            drained,
        );
    }
    let (Ok(commands), rest) = (commands, body.state()) else {
        return Flow::Close;
    };
    let payload = Payload::Pushed(&commands.raw, rest);
    forward(shared, client, up, head, &payload, Relay::Plain)
}

/// Records the refusal and answers it as git shows a hook's refusal, or with a plain 403 when the
/// push asked for no status report.
fn refuse(
    shared: &Shared,
    git: &GitSpec,
    client: &mut Client,
    (repo, commands): (&str, Option<&Commands>),
    refused: &Refused,
    drained: bool,
) -> Flow {
    let message = refused.message(&git.owned);
    shared.push_refused(PushRefusal {
        host: git.host.clone(),
        repo: repo
            .trim_start_matches('/')
            .trim_end_matches(".git")
            .to_owned(),
        git_ref: refused.git_ref().map(str::to_owned),
        reason: message.clone(),
    });
    let stream = client.get_mut();
    let sent = match commands
        .and_then(|c| report(c, &message))
        .filter(|_| drained)
    {
        Some(result) => write_result(stream, &result),
        None => write_refusal(stream, 403, &message),
    };
    drop(sent);
    stream.conn.send_close_notify();
    drop(stream.flush());
    Flow::Close
}

/// Writes a receive-pack result that ends the connection.
fn write_result(w: &mut impl Write, result: &[u8]) -> io::Result<()> {
    write!(
        w,
        "HTTP/1.1 200 OK\r\nContent-Type: application/x-git-receive-pack-result\r\n\
         Cache-Control: no-cache\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        result.len()
    )?;
    w.write_all(result)?;
    w.flush()
}
