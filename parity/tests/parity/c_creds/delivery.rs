use parity::adapter::{ExitClass, RunOpts, TaskState};
use parity::assert::{assert_never_called, assert_secret_absent};
use parity::parity;
use serde_json::{json, Value};

use super::support::*;
use crate::moor;

parity!(
    box_credentials_reach_setup_then_leave_no_trace,
    covers = ["C-127"],
    tags = [],
    {
        let f = fx();
        flow_fakes(&f.w);
        let (tok, keyf) = (f.secret("tok"), f.secret("keyf"));
        creds_config(&f, &tok, &keyf);
        f.repo_cfg(json!({ "setup": [SETUP_PROBE], "box_credentials": pair_credentials() }));
        let name = f.w.task_name("setup");
        let started = start_task(&f, &name, None, None);
        assert_eq!(started.class, ExitClass::Ok, "{}", started.stderr);
        let task = started.ok().clone();
        assert_eq!(task.state, TaskState::Working);
        assert_eq!(probe_file(&task, &f, "got-env"), tok.len().to_string());
        assert_eq!(probe_file(&task, &f, "got-file"), keyf.len().to_string());
        assert_eq!(probe_file(&task, &f, "got-filemode"), "600");
        assert_eq!(probe_file(&task, &f, "got-dirmode"), "700");

        let calls = f.w.all_calls();
        let setup_run = calls
            .iter()
            .find(|c| c.tool == "crabbox" && c.argv.iter().any(|a| a.contains("got-env")))
            .expect("setup run recorded");
        let delivered = calls
            .iter()
            .any(|c| c.tool == "ssh" && c.seq < setup_run.seq && c.stdin.contains(&tok));
        assert!(delivered, "credential reached the Box before setup ran");
        assert!(cred_dirs(&f.w).is_empty(), "{:?}", cred_dirs(&f.w));
        f.w.assert_no_new_tmp_creds();
        finish(&f, &name);
        for s in [&tok, &keyf] {
            assert_secret_absent(&f.w, s, &[]);
        }
    }
);

parity!(
    box_credentials_failing_setup_still_removes_dir,
    covers = ["C-127"],
    tags = [],
    {
        let f = fx();
        flow_fakes(&f.w);
        let (tok, keyf) = (f.secret("tok"), f.secret("keyf"));
        creds_config(&f, &tok, &keyf);
        f.repo_cfg(json!({
            "setup": ["test -n \"$TOK_V\"; exit 3"],
            "box_credentials": pair_credentials(),
        }));
        let name = f.w.task_name("badsetup");
        let started = start_task(&f, &name, None, None);
        let state = started.value.as_ref().map(|t| t.state.clone());
        assert_eq!(state, Some(TaskState::Working), "{}", started.stderr);
        assert!(f.w.calls("ssh").iter().any(|c| c.stdin.contains(&tok)));
        assert!(cred_dirs(&f.w).is_empty(), "{:?}", cred_dirs(&f.w));
        f.w.assert_no_new_tmp_creds();
        finish(&f, &name);
        for s in [&tok, &keyf] {
            assert_secret_absent(&f.w, s, &[]);
        }
    }
);

parity!(
    setup_without_box_credentials_runs_unwrapped,
    covers = ["C-127"],
    tags = [],
    {
        let f = fx();
        flow_fakes(&f.w);
        f.repo_cfg(json!({ "setup": ["echo plain-setup"] }));
        let name = f.w.task_name("plain");
        let started = start_task(&f, &name, None, None);
        assert_eq!(started.class, ExitClass::Ok, "{}", started.stderr);
        let setup = run_calls(&f.w)
            .into_iter()
            .find(|c| c.argv.iter().any(|a| a.contains("plain-setup")))
            .expect("setup run");
        assert!(!setup.argv.iter().any(|a| a.contains("moor-cred")));
        assert!(
            f.w.calls("ssh").is_empty(),
            "no delivery without box_credentials"
        );
        finish(&f, &name);
    }
);

parity!(
    run_with_missing_or_unreadable_source_delivers_nothing,
    covers = ["C-128"],
    tags = [],
    {
        let f = fx();
        flow_fakes(&f.w);
        let (tok, keyf) = (f.secret("tok"), f.secret("keyf"));
        creds_config(&f, &tok, &keyf);
        f.repo_cfg(json!({ "box_credentials": ["TOK"] }));
        let name = f.w.task_name("nosrc");
        assert_eq!(start_task(&f, &name, None, None).class, ExitClass::Ok);
        let attempt = |f: &Fx| {
            moor().run(
                &f.w,
                &f.repo.path,
                &name,
                &["echo", "marker-no-source"],
                &RunOpts::default(),
            )
        };
        f.user(json!({}));
        let missing = attempt(&f);
        assert_eq!(missing.class, ExitClass::Failed, "{}", missing.stderr);
        f.user(json!({ "credentials": { "TOK": { "file": "/nonexistent/tok" } } }));
        let unreadable = attempt(&f);
        assert_eq!(unreadable.class, ExitClass::Failed, "{}", unreadable.stderr);
        assert!(f.w.calls("ssh").is_empty(), "nothing delivered");
        let ran = run_calls(&f.w)
            .iter()
            .any(|c| c.argv.iter().any(|a| a.contains("marker-no-source")));
        assert!(!ran, "command must not run without its credential");
        finish(&f, &name);
    }
);

parity!(
    run_hides_credential_wrapper_echo,
    covers = ["C-129"],
    tags = [],
    {
        let f = fx();
        flow_fakes(&f.w);
        let (tok, keyf) = (f.secret("tok"), f.secret("keyf"));
        creds_config(&f, &tok, &keyf);
        f.repo_cfg(json!({ "box_credentials": ["TOK"] }));
        let name = f.w.task_name("wrap");
        assert_eq!(start_task(&f, &name, None, None).class, ExitClass::Ok);
        for quiet in [false, true] {
            let opts = RunOpts {
                quiet,
                ..RunOpts::default()
            };
            let o = moor().run(
                &f.w,
                &f.repo.path,
                &name,
                &["echo", "hello-from-box"],
                &opts,
            );
            assert_eq!(o.class, ExitClass::Ok, "quiet={quiet}: {}", o.stderr);
            let all = format!("{}{}", o.stdout, o.stderr);
            for bad in ["__moor_creds", "trap ", "export ", "moor-cred"] {
                assert!(
                    !all.contains(bad),
                    "quiet={quiet}: output shows {bad}: {all}"
                );
            }
            assert!(f.w.calls("ssh").iter().any(|c| c.stdin.contains(&tok)));
        }
        assert!(cred_dirs(&f.w).is_empty());
        finish(&f, &name);
        assert_secret_absent(&f.w, &tok, &[]);
    }
);

parity!(
    gh_token_box_credential_delivers_nothing,
    covers = ["C-130"],
    tags = [],
    {
        let f = fx();
        flow_fakes(&f.w);
        let secret = f.secret("ghbox");
        let file = f.secret_file("gh", &secret);
        f.user(json!({ "credentials": { "OTHER": { "file": file } } }));
        f.repo_cfg(
            json!({ "box_credentials": { "OTHER": { "env": "GH_TOKEN" } }, "setup": ["true"] }),
        );
        let name = f.w.task_name("ghtok");
        let started = start_task(&f, &name, None, None);
        assert_eq!(started.class, ExitClass::Failed, "{}", started.stdout);
        assert!(f.w.calls("ssh").is_empty(), "nothing delivered to a Box");
        assert!(run_calls(&f.w).is_empty(), "no Box script ran");
        assert_secret_absent(&f.w, &secret, &[]);
    }
);

parity!(
    start_rejects_proxied_claude_box_credential_before_side_effects,
    covers = ["C-15"],
    tags = [],
    {
        let f = fx();
        flow_fakes(&f.w);
        let secret = f.secret("claude");
        let file = f.secret_file("claude", &secret);
        f.user(json!({ "credentials": { "CLAUDE_CODE_OAUTH_TOKEN": { "file": file } } }));
        f.repo_cfg(json!({ "box_credentials": ["CLAUDE_CODE_OAUTH_TOKEN"] }));
        assert_accepted(&f.env(), "host location allows it");
        let name = f.w.task_name("claudebox");
        let started = start_task(&f, &name, Some("go"), Some("box"));
        let reason = serde_json::from_str::<Value>(&started.stdout)
            .ok()
            .and_then(|v| v["reason"].as_str().map(str::to_string))
            .unwrap_or_default();
        assert!(
            reason.contains("agent dispatch failed") && reason.contains("CLAUDE_CODE_OAUTH_TOKEN"),
            "start must report the rejected dispatch: {} {}",
            started.stdout,
            started.stderr
        );
        assert_never_called(&f.w.all_calls(), "herdr", &["pane", "run"]);
        assert!(f.w.calls("ssh").is_empty(), "no delivery");
        assert!(
            f.w.calls("crabbox")
                .iter()
                .all(|c| c.argv[0] != "tunnel" && c.argv[0] != "ssh"),
            "no proxy or delivery side effect"
        );
        assert!(!f.w.state_dir().join("proxy").exists(), "no proxy state");
        assert_secret_absent(&f.w, &secret, &[]);
        if started.value.is_some() {
            finish(&f, &name);
        }
    }
);

parity!(
    box_agent_pane_receives_no_box_credentials,
    covers = [],
    tags = ["new_only(BoxCredsHostOnly)"],
    {
        let f = fx();
        flow_fakes(&f.w);
        let (tok, keyf) = (f.secret("tok"), f.secret("keyf"));
        creds_config(&f, &tok, &keyf);
        f.repo_cfg(json!({
            "agent": { "location": "box" },
            "setup": [SETUP_PROBE],
            "box_credentials": pair_credentials(),
        }));
        let name = f.w.task_name("pane");
        let started = start_task(&f, &name, Some("go"), None);
        assert_eq!(started.class, ExitClass::Ok, "{}", started.stderr);
        let task = started.ok().clone();
        let setup_len = probe_file(&task, &f, "got-env");
        finish(&f, &name);

        assert_eq!(
            setup_len,
            tok.len().to_string(),
            "host-driven setup keeps its credential"
        );
        let calls = f.w.all_calls();
        let agent_dir = calls
            .iter()
            .filter(|c| c.tool == "ssh")
            .find_map(|c| {
                let cmd = c.argv.last()?;
                let at = cmd.find("proxy-nonce")?;
                cmd[..at].rsplit('\'').nth(1).map(str::to_string)
            })
            .expect("agent launch dir");
        for c in calls.iter().filter(|c| c.tool == "ssh") {
            let into_agent_dir = c.argv.last().is_some_and(|a| a.contains(&agent_dir));
            assert!(
                !(into_agent_dir && (c.stdin.contains(&tok) || c.stdin.contains(&keyf))),
                "agent launch dir received a box credential"
            );
        }
        for c in calls.iter().filter(|c| c.tool == "herdr") {
            let line = c.argv.join(" ");
            assert!(
                !line.contains("TOK_V") && !line.contains("KEYF_PATH"),
                "pane exports a credential"
            );
        }
    }
);
