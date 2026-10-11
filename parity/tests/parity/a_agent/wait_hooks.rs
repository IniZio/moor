use std::time::{Duration, Instant};

use parity::adapter::ExitClass;
use parity::fake::herdr;
use parity::parity;
use parity::world::hooks::HookState;

use super::support::*;

parity!(
    wait_idle_task_without_agent_returns,
    covers = ["A-85", "A-86"],
    tags = [],
    {
        let fx = box_fx();
        fx.start(false);
        fx.present();
        let o = fx.m.wait(&fx.w, &fx.repo.path, &fx.name, None);
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        let v = o.ok();
        assert!(!v.timed_out);
        assert!(v.run.is_none());
    }
);

parity!(
    wait_blocks_while_box_hook_says_working,
    covers = ["A-73", "A-74", "A-85", "A-86", "A-87", "REQ-02", "REQ-48"],
    tags = [],
    {
        let fx = box_fx();
        fx.start(true);
        fx.present();
        fx.w.hook_state(HookState::Working, 0);
        let began = Instant::now();
        let v = timed_out(&fx, 1);
        assert!(
            began.elapsed() < Duration::from_secs(6),
            "{:?}",
            began.elapsed()
        );
        let a = v.agent.expect("agent");
        assert_eq!(states(&a), (Some("working"), Some("hooks")));
    }
);

parity!(
    wait_returns_when_box_hook_says_idle,
    covers = ["A-73", "A-74", "A-85", "REQ-03", "REQ-05"],
    tags = [],
    {
        let fx = box_fx();
        fx.start(true);
        fx.present();
        fx.w.hook_state(HookState::Idle, 0);
        let o = fx.m.wait(&fx.w, &fx.repo.path, &fx.name, Some(5));
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        let a = o.ok().agent.clone().expect("agent");
        assert_eq!(states(&a), (Some("idle"), Some("hooks")));
        assert_eq!(a.state, "live");
    }
);

parity!(
    wait_background_marker_overrides_idle_hook,
    covers = ["A-74", "REQ-05"],
    tags = [],
    {
        let fx = box_fx();
        fx.start(true);
        fx.present();
        fx.w.hook_state(HookState::Idle, 0);
        herdr::pane_text(&fx.w, "  2 background tasks running\n> \n");
        assert_eq!(states(&fx.pane_agent()), (Some("working"), Some("pane")));
    }
);

parity!(
    wait_ends_when_box_agent_is_blocked,
    covers = ["A-74", "A-85"],
    tags = [],
    {
        let fx = box_fx();
        fx.start(true);
        fx.present();
        fx.w.hook_state(HookState::Blocked, 0);
        let o = fx.m.wait(&fx.w, &fx.repo.path, &fx.name, Some(5));
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        let a = o.ok().agent.clone().expect("agent");
        assert_eq!(states(&a), (Some("blocked"), Some("hooks")));
    }
);

parity!(
    box_agent_without_hook_falls_back_to_herdr_then_pane,
    covers = ["A-74", "REQ-05"],
    tags = [],
    {
        let fx = box_fx();
        fx.start(true);
        fx.present();
        let cases: [(Option<&str>, &str, (&str, &str)); 5] = [
            (Some("working"), "", ("working", "herdr")),
            (Some("blocked"), "", ("blocked", "herdr")),
            (Some("idle"), "all done\n", ("idle", "herdr")),
            (
                Some("idle"),
                "Cooked for 4s \u{b7} done\n> \n",
                ("idle", "pane"),
            ),
            (None, "thinking (esc to interrupt)\n", ("working", "pane")),
        ];
        for (status, text, want) in cases {
            herdr::pane_list(&fx.w, status);
            herdr::pane_text(&fx.w, text);
            let a = fx.pane_agent();
            assert_eq!(
                states(&a),
                (Some(want.0), Some(want.1)),
                "{status:?} {text:?}"
            );
        }
    }
);

parity!(
    exited_marker_ends_wait_and_reports_code,
    covers = ["A-71", "A-72", "A-75", "A-85", "A-86"],
    tags = [],
    {
        let fx = box_fx();
        fx.start(true);
        fx.present();
        let live = fx.pane_agent();
        assert_eq!(live.state, "live");
        assert_eq!(live.exit_code, None);
        herdr::pane_text(&fx.w, "bye\nmoor: agent exited 3\n");
        let o = fx.m.wait(&fx.w, &fx.repo.path, &fx.name, Some(5));
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        let a = o.ok().agent.clone().expect("agent");
        assert_eq!(a.state, "exited");
        assert_eq!(a.exit_code, Some(3));
        let p = fx.pane_agent();
        assert_eq!((p.state.as_str(), p.exit_code), ("exited", Some(3)));
        assert_eq!(fx.row().agent().expect("row agent").state, "exited");
    }
);

parity!(
    gone_pane_reports_gone_agent,
    covers = ["A-71", "A-72"],
    tags = [],
    {
        let fx = box_fx();
        fx.start(true);
        fx.present();
        herdr::pane_gone(&fx.w);
        assert_eq!(fx.pane_agent().state, "gone");
        let o = fx.m.wait(&fx.w, &fx.repo.path, &fx.name, Some(5));
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        assert_eq!(o.ok().agent.as_ref().expect("agent").state, "gone");
    }
);

parity!(
    failed_hook_read_uses_cache_only_while_fresh,
    covers = ["A-73", "REQ-03"],
    tags = [],
    {
        let fx = box_fx();
        fx.start(true);
        fx.present();
        fx.w.hook_state(HookState::Idle, 0);
        assert_eq!(states(&fx.wait_agent(5)), (Some("idle"), Some("hooks")));
        fx.w.clear_hook_state();
        herdr::pane_list(&fx.w, Some("working"));
        assert_eq!(
            states(&fx.pane_agent()),
            (Some("idle"), Some("hooks-cached"))
        );
        fx.m.age(&fx.w, &fx.name, 200);
        assert_eq!(states(&fx.pane_agent()), (Some("working"), Some("herdr")));
    }
);

parity!(
    hook_read_cut_at_deadline_is_unknown_never_idle,
    covers = ["A-73", "A-87", "REQ-02", "REQ-03", "REQ-52"],
    tags = [],
    {
        let fx = box_fx();
        fx.start(true);
        fx.present();
        fx.w.hook_state(HookState::Idle, 0);
        let hang = || {
            fx.w.prepend_rules(
                "ssh",
                [parity::fake::Rule::any()
                    .contains("agent-state")
                    .block()
                    .times(1)],
            )
        };
        hang();
        let began = Instant::now();
        let v = timed_out(&fx, 1);
        assert!(
            began.elapsed() < Duration::from_secs(6),
            "{:?}",
            began.elapsed()
        );
        assert_eq!(
            states(&v.agent.expect("agent")),
            (Some("unknown"), Some("timeout"))
        );
        assert_eq!(states(&fx.wait_agent(5)), (Some("idle"), Some("hooks")));
        hang();
        assert_eq!(
            states(&fx.wait_agent(5)),
            (Some("idle"), Some("hooks-cached"))
        );
    }
);

parity!(
    starting_hook_reads_working_only_while_young,
    covers = ["A-73", "REQ-07", "REQ-52"],
    tags = [],
    {
        let fx = box_fx();
        fx.start(true);
        fx.present();
        fx.w.hook_state(HookState::Starting, 0);
        herdr::pane_list(&fx.w, Some("idle"));
        let v = timed_out(&fx, 1);
        assert_eq!(
            states(&v.agent.expect("agent")),
            (Some("working"), Some("hooks"))
        );
        fx.m.age(&fx.w, &fx.name, 200);
        assert_eq!(states(&fx.wait_agent(5)), (Some("idle"), Some("herdr")));
    }
);

parity!(
    pane_wait_and_list_agree_on_agent_state,
    covers = ["A-70", "A-71", "REQ-04"],
    tags = [],
    {
        let fx = box_fx();
        fx.start(true);
        fx.present();
        for (hook, want) in [
            (HookState::Idle, ("idle", "hooks")),
            (HookState::Working, ("working", "hooks")),
            (HookState::Blocked, ("blocked", "hooks")),
        ] {
            fx.w.hook_state(hook, 0);
            let pane = fx.pane_agent();
            let list = fx.row().agent().expect("row agent");
            let wait = fx.m.wait(&fx.w, &fx.repo.path, &fx.name, Some(1));
            let wait = wait
                .value
                .clone()
                .and_then(|v| v.agent)
                .expect("wait agent");
            for a in [&pane, &list, &wait] {
                assert_eq!(states(a), (Some(want.0), Some(want.1)));
            }
        }
    }
);

parity!(
    one_hook_read_per_call,
    covers = ["REQ-04"],
    tags = ["new_only(SingleRead)"],
    {
        let fx = box_fx();
        fx.start(true);
        fx.present();
        fx.w.hook_state(HookState::Idle, 0);
        let before = hook_reads(&fx.w);
        fx.row();
        assert_eq!(
            hook_reads(&fx.w) - before,
            1,
            "list reads the hook file once"
        );
        let before = hook_reads(&fx.w);
        fx.pane_agent();
        assert_eq!(
            hook_reads(&fx.w) - before,
            1,
            "pane reads the hook file once"
        );
        let before = hook_reads(&fx.w);
        fx.wait_agent(5);
        assert_eq!(
            hook_reads(&fx.w) - before,
            1,
            "wait reads the hook file once"
        );
    }
);

parity!(
    box_launch_wires_claude_hooks_to_state_file,
    covers = ["REQ-05", "REQ-06"],
    tags = [],
    {
        let fx = box_fx();
        fx.start(true);
        let run =
            fx.w.calls("herdr")
                .into_iter()
                .find(|c| c.argv.starts_with(&["pane".to_string(), "run".to_string()]))
                .expect("agent pane run");
        let script = run.argv[3].replace(['\\', '\''], "");
        for pair in [
            r#""state":"working","event":"UserPromptSubmit""#,
            r#""state":"idle","event":"Stop""#,
            r#""state":"waiting_input","event":"Notification""#,
            r#""state":"waiting_input","event":"PreToolUse""#,
            r#""state":"working","event":"PostToolUse""#,
            r#""state":"starting""#,
        ] {
            assert!(script.contains(pair), "launch lacks {pair}");
        }
        for matcher in [
            "permission_prompt",
            "elicitation_dialog",
            "AskUserQuestion",
            "ExitPlanMode",
        ] {
            assert!(script.contains(matcher), "launch lacks matcher {matcher}");
        }
        assert!(
            !script.contains("idle_prompt"),
            "idle notification must not block"
        );
    }
);
