use super::*;

parity!(
    box_agent_preseeds_claude_first_run_state_for_the_workdir,
    covers = ["REQ-33"],
    tags = [],
    {
        let b = bx(false);
        let home_cfg = b.w.box_root().join("home/.claude.json");
        let wd = workdir(&b.w).display().to_string();
        for (tag, seeded) in [("fresh", false), ("merge", true)] {
            let _ = std::fs::remove_file(&home_cfg);
            if seeded {
                let seed = r#"{"keep":true,"projects":{"/other":{"a":1}}}"#;
                write_file(&home_cfg, seed, 0o600);
            }
            let name = b.w.task_name(tag);
            launched(&b, &name, "preseed");
            let r = ran(&b);
            assert_eq!(r.code, 0, "{}", r.text);
            let text = std::fs::read_to_string(&home_cfg).expect("~/.claude.json");
            let doc: Value = serde_json::from_str(&text).expect("json");
            assert_eq!(doc["hasCompletedOnboarding"], true, "{tag}");
            assert!(doc["theme"].is_string(), "{tag}");
            assert_eq!(
                doc["projects"][&wd]["hasTrustDialogAccepted"], true,
                "{tag}"
            );
            if seeded {
                assert_eq!(doc["keep"], true);
                assert_eq!(doc["projects"]["/other"]["a"], 1);
            }
            finish(&b, &name);
        }
    }
);

parity!(
    box_start_reports_first_run_prompt_without_failing,
    covers = ["A-40", "REQ-33"],
    tags = [],
    {
        let b = bx(false);
        let kinds = [
            (
                "trust",
                "Do you trust the files in this folder?",
                "folder_trust",
            ),
            ("enter", "Press Enter to continue", "press_enter"),
            ("notes", "Security notes:", "security_notes"),
            (
                "theme",
                "Choose the text style that looks best with your terminal",
                "theme",
            ),
        ];
        for (tag, line, kind) in kinds {
            herdr::pane_text(&b.w, &format!("welcome\n{line}\n"));
            let name = b.w.task_name(tag);
            let o = go(&b, &name, "prompt");
            assert_eq!(o.class, ExitClass::Ok, "{tag}: {}", o.stderr);
            assert_eq!(o.ok().state, TaskState::Working, "{tag}");
            let reason = reason_of(&o);
            let pane = &moor().sessions(&b.w, &name)[0].session_id;
            assert!(
                reason.contains(line) && reason.contains(kind) && reason.contains(pane.as_str()),
                "{tag}: {reason}"
            );
            finish(&b, &name);
        }
    }
);

parity!(
    box_start_with_quiet_or_unreadable_pane_still_succeeds_without_a_prompt_reason,
    covers = ["A-40"],
    tags = [],
    {
        let b = bx(false);
        herdr::pane_text(&b.w, "Welcome to Claude\n> \n");
        let quiet = b.w.task_name("quiet");
        let o = go(&b, &quiet, "no prompt");
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        assert_eq!(reason_of(&o), "");
        finish(&b, &quiet);
        b.w.prepend_rules(
            "herdr",
            [Rule::on(&["pane", "read"]).exit(1).stderr("no pane\n")],
        );
        let broken = b.w.task_name("broken");
        let o = go(&b, &broken, "read error");
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        assert_eq!(o.ok().state, TaskState::Working);
        assert_eq!(reason_of(&o), "");
        assert_eq!(moor().sessions(&b.w, &broken).len(), 1, "agent started");
        finish(&b, &broken);
    }
);

parity!(
    box_dispatch_failures_leave_the_task_working_and_a_rerun_retries,
    covers = ["A-41"],
    tags = [],
    {
        let b = bx(false);
        b.w.prepend_rules(
            "herdr",
            [Rule::on(&["tab", "create"])
                .exit(1)
                .stderr("tab refused\n")
                .times(1)],
        );
        let name = b.w.task_name("tabfail");
        let o = go(&b, &name, "dispatch");
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        assert_eq!(o.ok().state, TaskState::Working);
        let reason = reason_of(&o);
        assert!(
            reason.contains("agent dispatch failed") && reason.contains("re-run start"),
            "{reason}"
        );
        assert!(moor().sessions(&b.w, &name).is_empty());
        let again = go(&b, &name, "dispatch");
        assert_eq!(again.class, ExitClass::Ok, "{}", again.stderr);
        assert_eq!(reason_of(&again), "");
        assert_eq!(moor().sessions(&b.w, &name).len(), 1);
        finish(&b, &name);

        crabbox::deny(&b.w, "ssh", 1);
        let tabs = count(&b.w, "herdr", &["tab", "create"]);
        let blocked = b.w.task_name("nossh");
        let o = go(&b, &blocked, "dispatch");
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        assert_eq!(o.ok().state, TaskState::Working);
        assert!(
            reason_of(&o).contains("agent dispatch failed"),
            "{}",
            o.stdout
        );
        assert_eq!(
            count(&b.w, "herdr", &["tab", "create"]),
            tabs,
            "no pane without a channel"
        );
        assert!(moor().sessions(&b.w, &blocked).is_empty());
        finish(&b, &blocked);
    }
);

fn delivery_dir(c: &Call) -> Option<String> {
    let cmd = c.argv.last()?;
    let start = cmd.find("mkdir -p '")? + 10;
    let end = start + cmd[start..].find('\'')?;
    Some(cmd[start..end].trim_end_matches('/').to_string())
}

parity!(
    box_delivery_failure_removes_the_cred_dir_and_names_a_failed_cleanup,
    covers = ["B-116", "C-97", "A-41"],
    tags = [],
    {
        let b = bx(false);
        b.w.prepend_rules(
            "ssh",
            [Rule::any()
                .contains("brief.md")
                .exit(1)
                .stderr("disk full\n")],
        );
        let name = b.w.task_name("delfail");
        let o = go(&b, &name, "deliver");
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        assert_eq!(o.ok().state, TaskState::Working);
        let reason = reason_of(&o);
        assert!(
            reason.contains("agent dispatch failed") && reason.contains("brief.md"),
            "{reason}"
        );
        let ssh = b.w.calls("ssh");
        let first = &ssh[0];
        let remote = first.argv.last().cloned().unwrap_or_default();
        assert!(remote.contains("umask 077"), "{remote}");
        let dir = delivery_dir(first).expect("delivery dir");
        let slug = crabbox::slug(&b.w).expect("slug");
        let leaf = dir.rsplit('/').next().unwrap_or_default();
        let hex = leaf
            .strip_prefix(&format!("moor-cred-{slug}-"))
            .expect("dir naming");
        assert!(
            hex.len() == 16 && hex.chars().all(|c| c.is_ascii_hexdigit()),
            "{leaf}"
        );
        assert!(dir.starts_with("/tmp/"), "{dir}");
        let removal = ssh
            .last()
            .and_then(|c| c.argv.last())
            .cloned()
            .unwrap_or_default();
        assert!(
            removal.starts_with("rm -rf") && removal.contains(&dir),
            "{removal}"
        );
        assert!(
            cred_dirs(&b.w).is_empty(),
            "credential dir removed after the failure"
        );
        assert_never_called(&b.w.all_calls(), "herdr", &["pane", "run"]);
        finish(&b, &name);

        b.w.prepend_rules(
            "ssh",
            [Rule::any().contains("rm -rf").exit(1).stderr("busy\n")],
        );
        let stuck = b.w.task_name("rmfail");
        let o = go(&b, &stuck, "deliver");
        let reason = reason_of(&o);
        assert!(
            reason.contains("failed to remove box credential dir")
                && reason.contains("remove it by hand"),
            "{reason}"
        );
        assert_eq!(
            cred_dirs(&b.w).len(),
            1,
            "the dir the message names is still there"
        );
        finish(&b, &stuck);
    }
);

parity!(
    box_start_preflight_mints_and_discards_before_any_box_and_failure_is_retryable,
    covers = ["C-107"],
    tags = [],
    {
        let fresh = bx_with(true, false);
        let name = fresh.w.task_name("preflight");
        let o = go(&fresh, &name, "mint first");
        assert_eq!(o.class, ExitClass::Failed, "{}", o.stdout);
        assert!(
            o.stdout.trim().is_empty(),
            "stdout empty on preflight failure: {}",
            o.stdout
        );
        assert!(!o.stderr.trim().is_empty());
        let kinds: Vec<EventKind> = moor()
            .events(&fresh.w, &name)
            .into_iter()
            .map(|e| e.kind)
            .collect();
        assert!(kinds.contains(&EventKind::CredentialFailed), "{kinds:?}");
        let rows = moor().list(&fresh.w, &fresh.repo.path).ok().clone();
        assert_eq!(rows[0].state, TaskState::SetupFailed);
        assert_never_called(&fresh.w.all_calls(), "crabbox", &["warmup"]);
        assert_never_called(&fresh.w.all_calls(), "herdr", &["pane", "run"]);
        setup_app(&fresh);
        let o = go(&fresh, &name, "mint first");
        assert_eq!(o.class, ExitClass::Ok, "{}{}", o.stdout, o.stderr);
        assert_eq!(o.ok().state, TaskState::Working);
        assert_eq!(count(&fresh.w, "crabbox", &["warmup"]), 1);
        assert_eq!(count(&fresh.w, "herdr", &["pane", "run"]), 1);
        finish(&fresh, &name);

        let b = bx(true);
        let ok = b.w.task_name("minted");
        go_ok(&b, &ok, "mint first");
        assert!(scoped_mints(&b) >= 1, "a repo-scoped token was minted");
        for t in [&b.fx.probe_token, &b.fx.bot_token] {
            assert_secret_absent(&b.w, t, &["github/"]);
            assert!(!ssh_stdin_has(&b.w, t));
        }
        finish(&b, &ok);
    }
);

parity!(
    box_agent_starts_without_a_claude_token_configured,
    covers = ["C-109"],
    tags = [],
    {
        let b = bx(false);
        let name = b.w.task_name("notoken");
        let l = launched(&b, &name, "no token");
        let r = ran(&b);
        assert_eq!(r.code, 0, "{}", r.text);
        assert_eq!(r.env.get("CLAUDE_CODE_OAUTH_TOKEN"), Some(&l.nonce));
        assert_eq!(moor().sessions(&b.w, &name).len(), 1);
        finish(&b, &name);
    }
);

fn scoped_mints(b: &Bx) -> usize {
    let path = format!("/app/installations/{}/access_tokens", b.fx.installation_id);
    let scoped = |r: &parity::github::Request| r.body.contains("repository_ids");
    b.gh.requests_to("POST", &path)
        .iter()
        .filter(|r| scoped(r))
        .count()
}
