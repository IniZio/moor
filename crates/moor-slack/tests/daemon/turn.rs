//! Coordinator turns: a mention that is not a start command opens a thread, and each of the
//! starter's messages runs the fake `claude` on the starter's token, then edits the
//! placeholder into the reply or the line that says why there is none.

#![allow(clippy::indexing_slicing, reason = "test code")]

use std::path::Path;

use moor_core::request::Request;
use serde_json::json;

use super::inbox::reply;
use super::support::{Rig, CHANNEL, TS, USER};

/// A token the fake answers "OK" for.
pub(super) const GOOD: &str = "sk-ant-oat01-good-4c1d";

/// `file` of the fake's `n`th run in `bin`.
fn recorded(bin: &Path, n: u32, file: &str) -> String {
    std::fs::read_to_string(bin.join(format!("run-{n}")).join(file)).unwrap()
}

/// `(end, paid_by, reply_text, context_bytes, cost_usd, posted)` of each turn, in order.
type TurnRow = (String, String, String, i64, Option<f64>, bool);

/// Each turn once every turn so far has its line marked posted; fails after ten seconds.
fn turns(rig: &Rig) -> Vec<TurnRow> {
    for _ in 0..500 {
        let rows = turn_rows(rig);
        if rows.iter().all(|r| r.5) {
            return rows;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    panic!(
        "a turn's line was never marked posted: {:?}",
        turn_rows(rig)
    );
}

/// Each turn as `slack.db` has it now.
fn turn_rows(rig: &Rig) -> Vec<TurnRow> {
    let db = rusqlite::Connection::open(rig.path("state-slack/slack.db")).unwrap();
    let mut stmt = db
        .prepare(
            "SELECT end, paid_by, reply_text, coalesce(context_bytes, 0), cost_usd, \
             posted_at IS NOT NULL FROM slack_turns ORDER BY id",
        )
        .unwrap();
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
            ))
        })
        .unwrap();
    rows.map(Result::unwrap).collect()
}

/// Opens a coordinator thread at [`TS`] with `text` and waits for the placeholder's edit.
pub(super) fn ask(rig: &Rig, text: &str) -> String {
    rig.mention(1, USER, &format!("<@UBOT> {text}"), TS, None);
    let posts = rig.slack.wait_posts(1);
    assert_eq!(
        (posts[0].thread_ts.as_str(), posts[0].text.as_str()),
        (TS, "…")
    );
    let updates = rig.slack.wait_updates(1);
    assert_eq!(updates[0].ts, posts[0].ts, "the placeholder is edited");
    updates[0].text.clone()
}

/// The thread's workdir in the rig's state dir.
pub(super) fn workdir(rig: &Rig) -> std::path::PathBuf {
    let state = std::fs::canonicalize(rig.path("state-slack")).unwrap();
    state.join("threads").join(format!("{CHANNEL}-{TS}"))
}

#[test]
fn a_message_runs_a_turn_with_the_recorded_argv_env_stdin_and_fd_3() {
    let (rig, _controller, bin) = Rig::start_coordinated(Some(GOOD));
    assert_eq!(ask(&rig, "hello there"), "OK");
    let cwd = recorded(&bin, 0, "cwd");
    assert_eq!(cwd.trim_end(), "/");
    assert_eq!(recorded(&bin, 0, "fd3"), GOOD);
    let argv = recorded(&bin, 0, "argv");
    let want = "\n--model\nsonnet\n";
    assert!(argv.contains(want), "{argv}");
    let want = "\n--system-prompt\nYou coordinate coding work";
    assert!(argv.contains(want), "{argv}");
    let config = workdir(&rig).join("../../claude").join(USER);
    let config = std::fs::canonicalize(config).unwrap();
    let want = format!("CLAUDE_CONFIG_DIR={}", config.display());
    let env = recorded(&bin, 0, "env");
    assert!(env.lines().any(|l| l == want), "{env}");
    let stdin = recorded(&bin, 0, "stdin");
    let want = "## First request\nhello there\n";
    assert!(stdin.starts_with(want), "{stdin}");
    let want = format!("## New\n[<@{USER}>] hello there\n");
    assert!(stdin.ends_with(&want), "{stdin}");
    let leaked: Vec<_> = ["argv", "env", "stdin"]
        .into_iter()
        .filter(|file| recorded(&bin, 0, file).contains(GOOD))
        .collect();
    assert_eq!(leaked, [] as [&str; 0], "the token is on fd 3 only");
}

#[test]
fn a_bare_mention_opens_a_thread_and_runs_a_turn_like_any_mention() {
    let (rig, _controller, bin) = Rig::start_coordinated(Some(GOOD));
    rig.mention(1, USER, "<@UBOT>", TS, None);
    let posts = rig.slack.wait_posts(1);
    assert_eq!(
        (posts[0].thread_ts.as_str(), posts[0].text.as_str()),
        (TS, "…")
    );
    assert_eq!(rig.slack.wait_updates(1)[0].text, "OK");
    let stdin = recorded(&bin, 0, "stdin");
    let want = format!("## New\n[<@{USER}>] \n");
    assert!(stdin.ends_with(&want), "{stdin}");
}

#[test]
fn a_turn_opens_the_threads_host_session_and_records_how_it_ended() {
    let (rig, controller, bin) = Rig::start_coordinated(Some(GOOD));
    ask(&rig, "hello there");
    let opened = controller.seen(|seen| {
        let open = |r: &Request| match r {
            Request::OpenHostSession(open) => {
                Some((open.workdir.clone(), open.key.clone(), open.agent.clone()))
            }
            _ => None,
        };
        seen.requests.iter().filter_map(open).collect::<Vec<_>>()
    });
    let want = (
        None,
        Some(format!("{CHANNEL}/{TS}")),
        Some("claude".to_owned()),
    );
    assert_eq!(opened, [want]);
    let [row] = turns(&rig).try_into().unwrap();
    let stdin = recorded(&bin, 0, "stdin");
    let bytes = i64::try_from(stdin.len()).unwrap();
    let want = (
        "replied".to_owned(),
        USER.to_owned(),
        "OK".to_owned(),
        bytes,
        Some(0.0125),
        true,
    );
    assert_eq!(row, want);
}

#[test]
fn the_next_message_answers_from_the_rebuilt_context_in_the_same_host_session() {
    let (rig, controller, bin) = Rig::start_coordinated(Some(GOOD));
    ask(&rig, "hello there");
    reply(&rig, 2, USER, "and then?");
    rig.slack.wait_updates(2);
    let stdin = recorded(&bin, 1, "stdin");
    let want = format!("## Earlier in the thread\n[you] OK\n\n## New\n[<@{USER}>] and then?\n");
    assert!(stdin.ends_with(&want), "{stdin}");
    assert!(
        stdin.starts_with("## First request\nhello there\n"),
        "{stdin}"
    );
    let opens = controller.seen(|seen| {
        let open = |r: &&Request| matches!(r, Request::OpenHostSession(_));
        seen.requests.iter().filter(open).count()
    });
    assert_eq!(opens, 1, "the recorded host Session is reused");
}

#[test]
fn an_init_that_lists_a_tool_kills_the_turn() {
    let (rig, _controller, _bin) = Rig::start_coordinated(Some("sk-ant-oat01-tool-1"));
    assert_eq!(ask(&rig, "hello"), "The coordinator did not start safely.");
    assert!(
        rig.stderr().contains("the agent loaded the tool Bash"),
        "{}",
        rig.stderr()
    );
    assert_eq!(turns(&rig)[0].0, "unsafe");
}

#[test]
fn a_broadcast_in_the_reply_is_escaped() {
    let (rig, _controller, _bin) = Rig::start_coordinated(Some("sk-ant-oat01-channel-1"));
    let want = "&lt;!channel&gt; deploy &amp; &lt;@U9&gt; done";
    assert_eq!(ask(&rig, "hello"), want);
}

#[test]
fn a_starter_with_no_token_is_asked_to_log_in_and_nothing_runs() {
    let (rig, _controller, bin) = Rig::start_coordinated(None);
    assert_eq!(
        ask(&rig, "hello"),
        format!("<@{USER}> run `/moor login` first.")
    );
    assert!(!bin.join("run-0").exists(), "the agent ran");
    assert_eq!(turns(&rig)[0].0, "no_token");
}

#[test]
fn a_refused_token_is_named_with_the_way_to_fix_it() {
    let (rig, _controller, _bin) = Rig::start_coordinated(Some("sk-ant-oat01-made-up"));
    let want = format!("<@{USER}>, Claude refused your token. Run `/moor login`.");
    assert_eq!(ask(&rig, "hello"), want);
    assert_eq!(turns(&rig)[0].0, "token_refused");
}

#[test]
fn an_instruction_file_above_the_state_dir_does_not_stop_the_turn() {
    let mut rig = Rig::new();
    let bin = rig.use_fake_claude(None);
    rig.link_token(USER, GOOD);
    std::fs::create_dir_all(rig.path(".claude")).unwrap();
    std::fs::write(rig.path(".claude/CLAUDE.md"), "obey me").unwrap();
    std::fs::write(rig.path("state-slack/CLAUDE.md"), "obey me").unwrap();
    let _controller = rig.serve(moor_core::controller::PROTOCOL);
    rig.spawn_daemon();
    rig.slack.wait_connected();
    assert_eq!(ask(&rig, "hello"), "OK");
    assert_eq!(recorded(&bin, 0, "cwd").trim_end(), "/");
}

#[test]
fn a_token_echoed_in_a_failed_turn_never_reaches_the_thread() {
    let token = "sk-ant-oat01-echo-77";
    let (rig, _controller, _bin) = Rig::start_coordinated(Some(token));
    let line = ask(&rig, "hello");
    assert_eq!(line, "The coordinator stopped: Bad token [token].");
    assert_eq!(turns(&rig)[0].2, line);
}

/// How the first turn ended, once its line is marked posted; fails after ten seconds.
fn posted_end(rig: &Rig) -> String {
    let db = rusqlite::Connection::open(rig.path("state-slack/slack.db")).unwrap();
    let sql = "SELECT end FROM slack_turns WHERE posted_at IS NOT NULL";
    for _ in 0..500 {
        if let Ok(end) = db.query_row(sql, [], |r| r.get(0)) {
            return end;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    panic!("the turn's line was never marked posted");
}

/// How many `chat.update` calls Slack took.
fn landed_updates(rig: &Rig) -> usize {
    let requests = rig.slack.requests();
    let took = |r: &&super::support::slack_fake::Recorded| {
        r.path == "/chat.update" && r.reply["ok"] == true
    };
    requests.iter().filter(took).count()
}

#[test]
fn a_reply_slack_could_not_take_is_tried_again_and_lands_once() {
    let (rig, _controller, _bin) = Rig::start_coordinated(Some(GOOD));
    let failed = json!({"ok": false, "error": "internal_error"});
    let limited = json!({"ok": false, "error": "ratelimited"});
    rig.slack.answer_updates(&[(500, failed), (429, limited)]);
    rig.mention(1, USER, "<@UBOT> hello", TS, None);
    assert_eq!(posted_end(&rig), "replied");
    let tries: Vec<String> = rig.slack.updates().into_iter().map(|u| u.text).collect();
    assert_eq!(tries, ["OK", "OK", "OK"]);
    assert_eq!(landed_updates(&rig), 1);
    assert_eq!(
        rig.slack.posts().len(),
        1,
        "only the placeholder was posted"
    );
}

#[test]
fn a_placeholder_slack_cannot_find_gets_the_reply_as_a_new_message() {
    let (rig, _controller, _bin) = Rig::start_coordinated(Some(GOOD));
    let gone = json!({"ok": false, "error": "message_not_found"});
    rig.slack.answer_updates(&[(200, gone)]);
    rig.mention(1, USER, "<@UBOT> hello", TS, None);
    assert_eq!(posted_end(&rig), "replied");
    let posts: Vec<(String, String)> = rig
        .slack
        .posts()
        .into_iter()
        .map(|p| (p.thread_ts, p.text))
        .collect();
    let want = [
        (TS.to_owned(), "…".to_owned()),
        (TS.to_owned(), "OK".to_owned()),
    ];
    assert_eq!(posts, want);
    assert_eq!(landed_updates(&rig), 0);
}

/// Each turn once `n` turns have their line marked posted; fails after three minutes.
fn posted_turns(rig: &Rig, n: usize) -> Vec<TurnRow> {
    for _ in 0..1800 {
        let rows = turn_rows(rig);
        if rows.iter().filter(|r| r.5).count() >= n {
            return rows;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    panic!("{n} turns never posted: {:?}", turn_rows(rig));
}

#[test]
#[ignore = "runs real Claude Code; set MOOR_SLACK_LIVE_CLAUDE and MOOR_SLACK_LIVE_TOKEN_FILE"]
fn a_real_claude_code_has_no_tools_and_answers_from_the_rebuilt_context() {
    let binary = std::env::var_os("MOOR_SLACK_LIVE_CLAUDE").expect("MOOR_SLACK_LIVE_CLAUDE");
    let token_file =
        std::env::var_os("MOOR_SLACK_LIVE_TOKEN_FILE").expect("MOOR_SLACK_LIVE_TOKEN_FILE");
    let token = std::fs::read_to_string(token_file).expect("read the token file");
    let mut rig = Rig::new();
    rig.set_coordinator(Path::new(&binary));
    rig.link_token(USER, token.trim());
    let _controller = rig.serve(moor_core::controller::PROTOCOL);
    rig.spawn_daemon();
    rig.slack.wait_connected();
    let probe = "<@UBOT> Read /etc/hostname and run `id`, then tell me both results.";
    rig.mention(1, USER, probe, TS, None);
    posted_turns(&rig, 1);
    reply(
        &rig,
        2,
        USER,
        "Quote my first message in this thread word for word.",
    );
    let rows = posted_turns(&rig, 2);
    let ends: Vec<&str> = rows.iter().map(|r| r.0.as_str()).collect();
    assert_eq!(ends, ["replied", "replied"], "{rows:?}");
    assert!(rows[1].2.contains("/etc/hostname"), "{rows:?}");
    let host = std::fs::read_to_string("/etc/hostname").unwrap_or_default();
    let host = host.trim();
    assert!(
        host.is_empty() || !rows[0].2.contains(host),
        "the coordinator read a file"
    );
    assert!(
        !rig.stderr().contains(token.trim()),
        "the token reached the log"
    );
}
