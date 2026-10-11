use parity::adapter::ExitClass;
use parity::assert::{assert_never_called, assert_order};
use parity::fake::crabbox::Preset;
use parity::fake::herdr;
use parity::parity;

use super::support::*;

parity!(
    steer_host_resumes_the_same_session,
    covers = ["A-80", "A-81"],
    tags = [],
    {
        let fx = host_fx(Preset::NamespaceDevbox);
        fx.start(true);
        fx.present();
        let o = fx.m.steer(&fx.w, &fx.repo.path, &fx.name, "keep going");
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        let resume =
            fx.w.calls("claude")
                .into_iter()
                .find(|c| c.argv.first().map(String::as_str) == Some("--resume"))
                .expect("claude --resume");
        assert_eq!(resume.argv[1], "a1-full");
        assert!(resume.argv.iter().any(|a| a == "--bg"));
        assert_eq!(resume.argv.last().map(String::as_str), Some("keep going"));
        assert_never_called(&fx.w.calls("claude"), "claude", &["stop"]);
    }
);

parity!(
    steer_host_stops_a_live_session_before_resuming,
    covers = ["A-81"],
    tags = [],
    {
        let fx = host_fx(Preset::NamespaceDevbox);
        fx.start(true);
        fx.present();
        fx.w.prepend_rules(
            "claude",
            [parity::fake::Rule::on(&["agents"])
                .stdout(
                    r#"[{"id":"a1","sessionId":"a1-full","cwd":"","state":"working","pid":4242}]"#,
                )
                .times(2)],
        );
        let o = fx.m.steer(&fx.w, &fx.repo.path, &fx.name, "keep going");
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        assert_order(
            &fx.w.all_calls(),
            &[
                ("claude", &["stop", "a1"]),
                ("claude", &["--resume", "a1-full"]),
            ],
        );
    }
);

parity!(
    steer_host_refuses_a_blocked_agent,
    covers = ["A-81", "REQ-36"],
    tags = [],
    {
        let fx = host_fx(Preset::NamespaceDevbox);
        fx.start(true);
        fx.present();
        agents_with(&fx.w, "blocked", r#","waitingFor":"approval""#);
        let o = fx.m.steer(&fx.w, &fx.repo.path, &fx.name, "keep going");
        assert!(o.is_err());
        let calls = fx.w.calls("claude");
        assert_never_called(&calls, "claude", &["--resume"]);
        assert_never_called(&calls, "claude", &["stop"]);
        let a = fx.row().agent().expect("agent");
        assert_eq!(a.state, "blocked");
        assert_eq!(a.waiting_for.as_deref(), Some("approval"));
    }
);

parity!(
    steer_box_pane_types_message_and_submits,
    covers = ["A-80", "A-82", "REQ-36"],
    tags = [],
    {
        let fx = box_fx();
        fx.start(true);
        fx.present();
        let o = fx.m.steer(&fx.w, &fx.repo.path, &fx.name, "do more");
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        assert_order(
            &fx.w.all_calls(),
            &[
                ("herdr", &["pane", "send-keys", "p-1", "ctrl+u"]),
                ("herdr", &["pane", "send-text", "p-1", "do more"]),
                ("herdr", &["pane", "send-keys", "p-1", "ctrl+m"]),
            ],
        );
        assert_never_called(&fx.w.calls("claude"), "claude", &["--resume"]);
    }
);

parity!(
    steer_box_pane_refuses_gone_or_exited_pane,
    covers = ["A-82"],
    tags = [],
    {
        let fx = box_fx();
        fx.start(true);
        fx.present();
        herdr::pane_text(&fx.w, "bye\nmoor: agent exited 3\n");
        let exited = fx.m.steer(&fx.w, &fx.repo.path, &fx.name, "again");
        assert!(exited.is_err());
        herdr::pane_gone(&fx.w);
        let gone = fx.m.steer(&fx.w, &fx.repo.path, &fx.name, "again");
        assert!(gone.is_err());
        assert_never_called(&fx.w.calls("herdr"), "herdr", &["pane", "send-text"]);
    }
);

parity!(
    steer_needs_an_open_task_and_a_live_session,
    covers = ["A-80"],
    tags = [],
    {
        let fx = host_fx(Preset::NamespaceDevbox);
        fx.start(false);
        fx.present();
        let none = fx.m.steer(&fx.w, &fx.repo.path, &fx.name, "hello");
        assert!(none.is_err());
        let unknown = fx.m.steer(&fx.w, &fx.repo.path, "no-such-task", "hello");
        assert!(unknown.is_err());
        assert_never_called(&fx.w.calls("claude"), "claude", &["--resume"]);
    }
);

parity!(
    host_agent_state_reaches_list_row,
    covers = ["A-70", "A-71"],
    tags = [],
    {
        let fx = host_fx(Preset::NamespaceDevbox);
        fx.start(true);
        fx.present();
        for state in ["working", "done", "failed", "stopped"] {
            agents(&fx.w, state);
            let a = fx.row().agent().expect("agent");
            assert_eq!(a.state, state);
            assert_eq!(a.waiting_for, None);
        }
        agents_with(&fx.w, "blocked", r#","waitingFor":"approval""#);
        let a = fx.row().agent().expect("agent");
        assert_eq!(a.state, "blocked");
        assert_eq!(a.waiting_for.as_deref(), Some("approval"));
    }
);
