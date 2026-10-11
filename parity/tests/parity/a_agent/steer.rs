use parity::adapter::ExitClass;
use parity::assert::{assert_never_called, assert_order};
use parity::fake::crabbox::Preset;
use parity::fake::herdr;
use parity::parity;
use parity::world::hooks::HookState;

use super::support::*;

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
        let fx = box_fx_on(Preset::NamespaceDevbox);
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
    steer_to_blocked_box_agent_answers_the_prompt,
    covers = ["REQ-36"],
    tags = [],
    {
        let fx = box_fx();
        fx.start(true);
        fx.present();
        fx.w.hook_state(HookState::Blocked, 0);
        herdr::pane_text(&fx.w, "Proceed?\n 1. Yes\n 2. No\n");
        let o = fx.m.steer(&fx.w, &fx.repo.path, &fx.name, "1");
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        let sent = fx.w.calls("herdr");
        let typed = sent.iter().any(|c| {
            c.argv
                .starts_with(&["pane".into(), "send-text".into(), "p-1".into(), "1".into()])
        });
        assert!(typed, "prompt answer not sent: {sent:?}");
    }
);
