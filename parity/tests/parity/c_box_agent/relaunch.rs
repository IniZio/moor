use super::*;

fn live_panes(w: &World) {
    let panes: Vec<Value> = (1..=9)
        .map(|n| json!({ "pane_id": format!("p-{n}") }))
        .collect();
    let body = json!({ "result": { "panes": panes } }).to_string();
    w.prepend_rules("herdr", [Rule::on(&["pane", "list"]).stdout(body)]);
}

parity!(
    box_start_attaches_to_a_live_pane_and_ignores_the_new_goal,
    covers = ["A-48", "C-110"],
    tags = [],
    {
        let b = bx(false);
        let name = b.w.task_name("live");
        go_ok(&b, &name, "first goal");
        let again = go(&b, &name, "second goal");
        assert_eq!(again.class, ExitClass::Ok, "{}", again.stderr);
        let d = doc(&again);
        assert_eq!(d["attached"], true, "{d}");
        assert!(d["relaunched"].is_null(), "{d}");
        assert!(again.stderr.contains("--goal ignored"), "{}", again.stderr);
        assert_eq!(count(&b.w, "herdr", &["tab", "create"]), 1);
        assert_eq!(moor().sessions(&b.w, &name).len(), 1);
        assert_eq!(count(&b.w, "crabbox", &["tunnel"]), 1);
        finish(&b, &name);
    }
);

parity!(
    box_start_relaunches_a_gone_pane_with_new_goal_saving_the_transcript_first,
    covers = ["A-46", "A-47", "C-110"],
    tags = [],
    {
        let b = bx(false);
        crabbox::cp(&b.w);
        crabbox::run_in(&b.w, &workdir(&b.w));
        seed_transcript(&b.w);
        let name = b.w.task_name("gone");
        go_ok(&b, &name, "first goal");
        let old = moor().sessions(&b.w, &name)[0].session_id.clone();
        herdr::pane_gone(&b.w);
        let o = go(&b, &name, "second goal");
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        let d = doc(&o);
        assert_eq!(d["relaunched"]["goal"], "second goal", "{d}");
        assert_eq!(d["relaunched"]["old_pane"], old.as_str(), "{d}");
        let new = d["relaunched"]["new_pane"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(!new.is_empty() && new != old);
        let sessions = moor().sessions(&b.w, &name);
        let live: Vec<_> = sessions.iter().filter(|s| s.live).collect();
        assert_eq!(live.len(), 1, "{sessions:?}");
        assert_eq!(live[0].session_id, new);
        assert!(live[0].box_pane);
        assert_eq!(
            count(&b.w, "crabbox", &["tunnel"]),
            1,
            "relaunch reuses the proxy"
        );
        assert_order(
            &b.w.all_calls(),
            &[
                ("herdr", &["tab", "create"]),
                ("crabbox", &["run"]),
                ("crabbox", &["cp"]),
                ("herdr", &["tab", "create"]),
            ],
        );
        let archive =
            b.w.calls("crabbox")
                .into_iter()
                .find(|c| c.argv.iter().any(|a| a.contains("transcripts.tgz")))
                .expect("transcript archive run");
        assert!(archive.argv.iter().any(|a| a == "--no-sync"));
        finish(&b, &name);
    }
);

parity!(
    box_relaunch_uses_the_newest_goal,
    covers = ["A-46"],
    tags = [],
    {
        let b = bx(false);
        live_panes(&b.w);
        let name = b.w.task_name("goals");
        go_ok(&b, &name, "goal one");
        herdr::pane_text(&b.w, "bye\nmoor: agent exited 3\n");
        let attach = |goal: Option<&str>| {
            let o = start_with(&b, &name, &start_opts(goal, None));
            assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
            doc(&o)["relaunched"].clone()
        };
        let first = attach(None);
        assert_eq!(first["goal"], "goal one", "{first}");
        assert_eq!(first["old_pane"], "p-1");
        let second = attach(Some("goal two"));
        assert_eq!(second["goal"], "goal two", "{second}");
        assert_eq!(second["old_pane"], first["new_pane"]);
        let third = attach(None);
        assert_eq!(third["goal"], "goal two", "{third}");
        assert_eq!(count(&b.w, "herdr", &["tab", "create"]), 4);
        finish(&b, &name);
    }
);

parity!(
    box_probe_errors_count_as_a_live_pane_and_block_relaunch,
    covers = ["A-46", "A-48"],
    tags = [],
    {
        let b = bx(false);
        let name = b.w.task_name("flaky");
        go_ok(&b, &name, "first goal");
        b.w.prepend_rules(
            "herdr",
            [
                Rule::on(&["pane", "list"]).exit(1).stderr("herdr down\n"),
                Rule::on(&["pane", "read"]).exit(1).stderr("herdr down\n"),
            ],
        );
        let o = go(&b, &name, "second goal");
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        let d = doc(&o);
        assert!(d["relaunched"].is_null(), "{d}");
        assert_eq!(count(&b.w, "herdr", &["tab", "create"]), 1);
        assert_eq!(moor().sessions(&b.w, &name).len(), 1);
        finish(&b, &name);
    }
);

parity!(
    box_steer_on_an_exited_pane_names_the_start_command_to_fix_it,
    covers = ["REQ-35"],
    tags = [],
    {
        let b = bx(false);
        live_panes(&b.w);
        let name = b.w.task_name("steerdead");
        go_ok(&b, &name, "first goal");
        herdr::pane_text(&b.w, "bye\nmoor: agent exited 3\n");
        let o = moor().steer(&b.w, &b.repo.path, &name, "again");
        assert_eq!(o.class, ExitClass::Failed, "{}", o.stderr);
        for want in [name.as_str(), "start", "--goal", "3"] {
            assert!(o.stderr.contains(want), "{want} missing from: {}", o.stderr);
        }
        assert_never_called(&b.w.all_calls(), "herdr", &["pane", "send-text"]);
        let again = start_with(&b, &name, &start_opts(Some("again"), None));
        assert_eq!(again.class, ExitClass::Ok, "{}", again.stderr);
        assert_eq!(
            count(&b.w, "herdr", &["tab", "create"]),
            2,
            "new pane opened"
        );
        assert_eq!(
            moor()
                .sessions(&b.w, &name)
                .iter()
                .filter(|s| s.live)
                .count(),
            1
        );
        finish(&b, &name);
    }
);

fn seed_transcript(w: &World) {
    let wd = workdir(w).display().to_string();
    let enc: String = wd
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let rel = format!("home/.claude/projects/{enc}/session1.jsonl");
    write_file(&w.box_root().join(rel), "{\"type\":\"user\"}\n", 0o644);
}

fn transcripts(w: &World) -> Vec<PathBuf> {
    files_under(&w.state_dir())
        .into_iter()
        .filter(|f| {
            f.to_string_lossy().contains("-transcript-")
                && f.extension().is_some_and(|e| e == "tgz")
        })
        .collect()
}

parity!(
    box_close_and_relaunch_save_the_transcript_0600_and_failure_never_blocks,
    covers = ["A-47", "REQ-37"],
    tags = [],
    {
        let b = bx(false);
        crabbox::cp(&b.w);
        crabbox::run_in(&b.w, &workdir(&b.w));
        seed_transcript(&b.w);
        let name = b.w.task_name("transcript");
        go_ok(&b, &name, "first goal");
        herdr::pane_gone(&b.w);
        assert!(!doc(&go(&b, &name, "second goal"))["relaunched"].is_null());
        let saved = transcripts(&b.w);
        assert_eq!(saved.len(), 1, "relaunch saved one archive: {saved:?}");
        assert_eq!(mode_of(&saved[0]), 0o600);
        let tar = b.w.command("tar").arg("tzf").arg(&saved[0]).output();
        let listed = String::from_utf8_lossy(&tar.expect("tar").stdout).into_owned();
        assert!(listed.contains("session1.jsonl"), "{listed}");
        let closed = moor().close(&b.w, &b.repo.path, &name, false);
        assert_eq!(closed.class, ExitClass::Ok, "{}", closed.stderr);
        wait_proxy_gone(&b.w);
        let saved = transcripts(&b.w);
        assert_eq!(saved.len(), 2, "close saved a second archive: {saved:?}");
        assert!(saved.iter().all(|f| mode_of(f) == 0o600));
        let cp = crabbox::call(&b.w, "cp").expect("cp");
        assert!(cp
            .argv
            .iter()
            .any(|a| a.starts_with("SANDBOX:") && a.ends_with(".moor/transcripts.tgz")));
        assert_order(
            &b.w.all_calls(),
            &[("crabbox", &["cp"]), ("crabbox", &["stop"])],
        );

        std::fs::remove_dir_all(b.w.box_root().join("home/.claude")).expect("clear transcripts");
        let bare = b.w.task_name("notrans");
        go_ok(&b, &bare, "no transcript");
        let closed = moor().close(&b.w, &b.repo.path, &bare, false);
        assert_eq!(closed.class, ExitClass::Ok, "{}", closed.stderr);
        wait_proxy_gone(&b.w);
        assert_eq!(
            transcripts(&b.w).len(),
            2,
            "nothing saved without a transcript"
        );
        assert!(closed.ok().box_deleted, "close still released the Box");
    }
);

fn alive(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|s| {
            s.rsplit(')')
                .next()
                .map(|t| t.trim_start().starts_with(['R', 'S', 'D']))
        })
        .unwrap_or(false)
}

parity!(
    box_close_ends_the_proxy_supervisor_and_its_tunnel,
    covers = ["A-96"],
    tags = [],
    {
        let b = bx(false);
        let name = b.w.task_name("proxyend");
        go_ok(&b, &name, "tunnel up");
        let tunnel = crabbox::call(&b.w, "tunnel").expect("tunnel");
        assert!(alive(tunnel.pid), "tunnel runs while the Task is open");
        finish(&b, &name);
        let deadline = Instant::now() + Duration::from_secs(10);
        while alive(tunnel.pid) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(!alive(tunnel.pid), "tunnel survived close");
    }
);

fn saved_digests(w: &World) -> Vec<String> {
    transcripts(w)
        .iter()
        .map(|f| {
            let out = w.command("sha256sum").arg(f).output().expect("sha256sum");
            String::from_utf8_lossy(&out.stdout)
                .split_whitespace()
                .next()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

parity!(
    box_transcript_is_saved_when_the_exit_marker_is_observed_and_again_never_duplicated,
    covers = ["REQ-37"],
    tags = ["new_only(TranscriptOnExit)"],
    {
        let b = bx(false);
        crabbox::cp(&b.w);
        crabbox::run_in(&b.w, &workdir(&b.w));
        live_panes(&b.w);
        seed_transcript(&b.w);
        let name = b.w.task_name("exitsave");
        go_ok(&b, &name, "first goal");
        assert!(transcripts(&b.w).is_empty(), "nothing saved while running");
        herdr::pane_text(&b.w, "bye\nmoor: agent exited 3\n");
        let seen = moor().pane(&b.w, &b.repo.path, &name);
        assert_eq!(seen.class, ExitClass::Ok, "{}", seen.stderr);
        let saved = transcripts(&b.w);
        assert_eq!(saved.len(), 1, "exit saved one archive: {saved:?}");
        assert_eq!(mode_of(&saved[0]), 0o600);
        let tar = b.w.command("tar").arg("tzf").arg(&saved[0]).output();
        let listed = String::from_utf8_lossy(&tar.expect("tar").stdout).into_owned();
        assert!(listed.contains("session1.jsonl"), "{listed}");
        let again = moor().pane(&b.w, &b.repo.path, &name);
        assert_eq!(again.class, ExitClass::Ok, "{}", again.stderr);
        assert_eq!(transcripts(&b.w).len(), 1, "observing twice saves once");
        finish(&b, &name);
    }
);

parity!(
    box_close_after_an_exit_save_does_not_write_a_second_identical_transcript,
    covers = ["REQ-37"],
    tags = ["new_only(TranscriptDedup)"],
    {
        let b = bx(false);
        crabbox::cp(&b.w);
        crabbox::run_in(&b.w, &workdir(&b.w));
        live_panes(&b.w);
        seed_transcript(&b.w);
        let name = b.w.task_name("dedup");
        go_ok(&b, &name, "first goal");
        herdr::pane_text(&b.w, "bye\nmoor: agent exited 0\n");
        let seen = moor().pane(&b.w, &b.repo.path, &name);
        assert_eq!(seen.class, ExitClass::Ok, "{}", seen.stderr);
        let before = saved_digests(&b.w);
        assert_eq!(before.len(), 1, "exit saved one archive: {before:?}");
        let closed = moor().close(&b.w, &b.repo.path, &name, false);
        assert_eq!(closed.class, ExitClass::Ok, "{}", closed.stderr);
        wait_proxy_gone(&b.w);
        assert_eq!(saved_digests(&b.w), before, "close added no duplicate");
        assert!(closed.ok().box_deleted, "close still released the Box");
    }
);
