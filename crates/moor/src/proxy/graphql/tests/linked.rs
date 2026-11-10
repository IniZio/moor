//! A linked issue's title and state reach the Box only when its repo is a Box repo.

use std::sync::Condvar;

use super::*;

/// An issue of the Box repo with its linked issues, the way `gh issue view` reads them.
const ISSUE: &str = "query($owner: String!, $repo: String!) { repository(owner: $owner, name: $repo) { \
     issue: issueOrPullRequest(number: 1) { ... on Issue { title state \
     parent { id number title url state repository { nameWithOwner } } \
     subIssues(first: 10) { nodes { number title state repository { nameWithOwner } } } \
     blocking(first: 10) { edges { node { number title state repository { nameWithOwner } } } } } } } }";

/// A linked issue in `repo`, with the headline `title`.
fn linked(repo: &str, title: &str) -> Value {
    json!({"number": 7, "title": title, "state": "OPEN", "repository": {"nameWithOwner": repo}})
}

/// The request for `query` on the Box repo.
fn ask(query: &str) -> Vec<u8> {
    let body = json!({"query": query, "variables": {"owner": "IniZio", "repo": "moor"}});
    post("/graphql", &body.to_string())
}

/// The JSON body of a response.
fn body(resp: &Wire) -> Value {
    serde_json::from_slice(&resp.body).unwrap()
}

/// A filter for a Box of `IniZio/moor`, started with `--also IniZio/lib`, whose upstream
/// answers every request with `data`.
fn rig_answering(data: &Value) -> Rig {
    let reply = json_reply("200 OK", &json!({ "data": data }).to_string());
    rig_for(
        fixed(reply),
        Guard::default(),
        vec![box_repo(), RepoRef::new("IniZio", "lib")],
    )
}

#[test]
fn a_linked_issue_in_a_box_repo_keeps_its_title_and_state() {
    let data = json!({"repository": {"issue": {
        "title": "own", "state": "OPEN",
        "parent": linked("IniZio/moor", "primary parent"),
        "subIssues": {"nodes": [linked("IniZio/lib", "also sub")]},
        "blocking": {"edges": [{"node": linked("IniZio/lib", "also blocked")}]},
    }}});
    let rig = rig_answering(&data);
    let resp = rig.client().call(&ask(ISSUE));
    assert_eq!(resp.status(), 200, "{}", resp.head);
    assert_eq!(body(&resp), json!({ "data": data }));
}

#[test]
fn a_linked_issue_in_another_repo_has_its_title_and_state_blanked() {
    let data = json!({"repository": {"issue": {
        "title": "own", "state": "OPEN",
        "parent": linked("victim/secrets", "hidden parent"),
        "subIssues": {"nodes": [linked("IniZio/lib", "also sub"), linked("victim/secrets", "hidden sub")]},
        "blocking": {"edges": [{"node": linked("IniZio/other", "hidden blocked")}]},
    }}});
    let rig = rig_answering(&data);
    let resp = rig.client().call(&ask(ISSUE));
    assert_eq!(resp.status(), 200, "{}", resp.head);
    let text = String::from_utf8_lossy(&resp.body).into_owned();
    assert!(!text.contains("hidden"), "{text}");
    let blank = |repo: &str| json!({"number": 7, "title": null, "state": null, "repository": {"nameWithOwner": repo}});
    let want = json!({"repository": {"issue": {
        "title": "own", "state": "OPEN",
        "parent": blank("victim/secrets"),
        "subIssues": {"nodes": [linked("IniZio/lib", "also sub"), blank("victim/secrets")]},
        "blocking": {"edges": [{"node": blank("IniZio/other")}]},
    }}});
    assert_eq!(body(&resp), json!({ "data": want }));
}

#[test]
fn a_linked_issue_without_its_repository_is_blanked() {
    let query =
        "query($owner: String!, $repo: String!) { repository(owner: $owner, name: $repo) { \
         issue(number: 1) { parent { number title state } \
         p: parent { title repository @skip(if: true) { nameWithOwner } } } } }";
    let data = json!({"repository": {"issue": {
        "parent": {"number": 7, "title": "hidden parent", "state": "OPEN"},
        "p": {"title": "secret alias"},
    }}});
    let rig = rig_answering(&data);
    let resp = rig.client().call(&ask(query));
    assert_eq!(resp.status(), 200, "{}", resp.head);
    let want = json!({"repository": {"issue": {
        "parent": {"number": 7, "title": null, "state": null},
        "p": {"title": null},
    }}});
    assert_eq!(body(&resp), json!({ "data": want }));
}

#[test]
fn repo_names_are_compared_without_case() {
    let data = json!({"repository": {"issue": {
        "parent": linked("inizio/MOOR", "primary parent"),
        "subIssues": {"nodes": [linked("INIZIO/Lib", "also sub")]},
    }}});
    let rig = rig_answering(&data);
    let resp = rig.client().call(&ask(ISSUE));
    assert_eq!(body(&resp), json!({ "data": data }));
}

#[test]
fn a_checked_response_is_asked_for_unencoded_and_refused_when_encoded() {
    let reply = b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\n\
        content-encoding: gzip\r\ncontent-length: 4\r\n\r\nabcd"
        .to_vec();
    let rig = rig_for(fixed(reply), Guard::default(), vec![box_repo()]);
    let mut sent = ask(ISSUE);
    let at = sent.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    sent.splice(at..at, b"\r\naccept-encoding: gzip".iter().copied());
    let resp = rig.client().call(&sent);
    assert_refused(&resp, 502);
    let seen = rig.requests();
    assert_eq!(
        seen.first().unwrap().header("accept-encoding"),
        Some("identity")
    );
}

#[test]
fn a_query_without_linked_issues_is_relayed_as_sent() {
    let reply = chunked_reply(&[r#"{"data":"#, r#"{"ok":true}}"#]);
    let rig = rig_for(fixed(reply.clone()), Guard::default(), vec![box_repo()]);
    let sent = ask("query($owner: String!, $repo: String!) { repository(owner: $owner, name: $repo) { issue(number: 1) { title } } }");
    let resp = rig.client().call(&sent);
    assert_eq!(resp.raw, reply);
    assert_eq!(rig.requests().first().unwrap().raw, sent);
}

/// The largest checked response body, enough for `gh issue view --comments`: 100
/// comments of up to 64 KiB and 100 sub-issues.
const CAP: usize = 8 * 1024 * 1024;

/// A response of `size` bytes, a JSON body padded by one long string.
fn reply_of(size: usize) -> Vec<u8> {
    let frame = r#"{"data":{"pad":""}}"#.len();
    let pad = "x".repeat(size - frame);
    let body = json!({"data": {"pad": pad}}).to_string();
    assert_eq!(body.len(), size);
    json_reply("200 OK", &body)
}

#[test]
fn a_checked_response_over_the_cap_is_refused() {
    let rig = rig_for(fixed(reply_of(CAP + 1)), Guard::default(), vec![box_repo()]);
    assert_refused(&rig.client().call(&ask(ISSUE)), 502);
}

#[test]
fn a_checked_response_at_the_cap_is_relayed() {
    let rig = rig_for(fixed(reply_of(CAP)), Guard::default(), vec![box_repo()]);
    let resp = rig.client().call(&ask(ISSUE));
    assert_eq!(resp.status(), 200, "{}", resp.head);
    assert_eq!(resp.body.len(), CAP);
}

#[test]
fn a_second_checked_response_waits_while_the_first_is_in_flight() {
    let arrived = Arc::new(AtomicUsize::new(0));
    let held = Arc::new((Mutex::new(true), Condvar::new()));
    let reply = json_reply("200 OK", r#"{"data":{"repository":null}}"#);
    let (seen, gate) = (Arc::clone(&arrived), Arc::clone(&held));
    let rig = rig_for(
        Arc::new(move |_| {
            if seen.fetch_add(1, Ordering::SeqCst) == 0 {
                let (lock, freed) = &*gate;
                let held = lock.lock().unwrap_or_else(PoisonError::into_inner);
                drop(freed.wait_while(held, |h| *h));
            }
            reply.clone()
        }),
        Guard::default(),
        vec![box_repo()],
    );
    let mut first = rig.client();
    first.send(&ask(ISSUE)).unwrap();
    for _ in 0..100 {
        if arrived.load(Ordering::SeqCst) > 0 {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let mut second = rig.client();
    let second = std::thread::spawn(move || second.call(&ask(ISSUE)));
    std::thread::sleep(Duration::from_millis(500));
    let during = arrived.load(Ordering::SeqCst);
    *held.0.lock().unwrap_or_else(PoisonError::into_inner) = false;
    held.1.notify_all();
    assert_eq!(during, 1, "the second checked request went upstream early");
    assert_eq!(first.recv().unwrap().status(), 200);
    assert_eq!(second.join().unwrap().status(), 200);
    assert_eq!(arrived.load(Ordering::SeqCst), 2);
}
