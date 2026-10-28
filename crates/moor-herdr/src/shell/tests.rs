use std::cell::Cell;
use std::collections::BTreeMap;

use super::{decide, host_shell, Pane, Target};
use crate::boxes::{Activity, Phase, Shown};
use crate::error::Error;
use crate::herdr::Space;

fn space(id: &str, label: &str) -> Space {
    Space {
        id: id.to_owned(),
        label: label.to_owned(),
        tokens: BTreeMap::new(),
        agent_status: "unknown".to_owned(),
    }
}

fn spaces() -> Vec<Space> {
    vec![space("w1", "moor"), space("w2", "◈ moor/fix-login")]
}

/// Decides for `p` and counts the herdr and controller calls it made.
fn counted(p: Pane<'_>) -> (Target, u32) {
    let calls = Cell::new(0);
    let got = decide(
        p,
        || {
            calls.set(calls.get() + 1);
            Ok(spaces())
        },
        |_| {
            calls.set(calls.get() + 1);
            Ok(None)
        },
    );
    (got, calls.get())
}

fn shown(phase: Phase) -> Shown {
    Shown {
        name: "fix-login".to_owned(),
        repo: Some("acme/moor".to_owned()),
        branch: None,
        phase,
    }
}

const fn pane(workspace: Option<&str>) -> Pane<'_> {
    Pane {
        workspace,
        agent: false,
        socket: true,
    }
}

fn decide_with(p: Pane<'_>, box_phase: Option<Phase>) -> Target {
    decide(
        p,
        || Ok(spaces()),
        |name| {
            assert_eq!(name, "fix-login");
            Ok(box_phase.map(shown))
        },
    )
}

#[test]
fn a_live_box_workspace_opens_a_shell_in_that_box() {
    let live = Some(Phase::Live(Activity::Idle));
    assert_eq!(
        decide_with(pane(Some("w2")), live),
        Target::Box("fix-login".to_owned())
    );
}

#[test]
fn a_plain_workspace_gets_the_host_shell_without_asking_the_controller() {
    assert_eq!(counted(pane(Some("w1"))), (Target::Host, 1));
}

#[test]
fn a_pane_outside_herdr_or_without_a_controller_skips_every_lookup() {
    let no_socket = Pane {
        socket: false,
        ..pane(Some("w2"))
    };
    for p in [pane(None), no_socket] {
        assert_eq!(counted(p), (Target::Host, 0));
    }
}

#[test]
fn the_pane_sync_drives_stays_a_host_shell() {
    let agent = Pane {
        agent: true,
        ..pane(Some("w2"))
    };
    assert_eq!(counted(agent), (Target::Host, 0));
}

#[test]
fn a_box_that_cannot_take_a_shell_falls_back_with_the_reason() {
    let cases = [
        (Some(Phase::Asleep), "Box fix-login is asleep"),
        (Some(Phase::Starting), "Box fix-login is still being set up"),
        (None, "Box fix-login is closed"),
    ];
    for (phase, why) in cases {
        assert_eq!(
            decide_with(pane(Some("w2")), phase),
            Target::Fallback(why.to_owned())
        );
    }
}

#[test]
fn an_unreachable_controller_falls_back_with_the_reason() {
    let got = decide(
        pane(Some("w2")),
        || Ok(spaces()),
        |_| Err(Error::Refused("down".to_owned())),
    );
    assert_eq!(
        got,
        Target::Fallback("Box fix-login could not be reached: controller: down".to_owned())
    );
}

#[test]
fn the_host_shell_is_the_recorded_one_then_shell_then_sh() {
    let s = |v: &str| Some(v.to_owned());
    assert_eq!(host_shell(s("/bin/zsh"), s("/bin/bash")), "/bin/zsh");
    assert_eq!(host_shell(None, s("/bin/bash")), "/bin/bash");
    assert_eq!(host_shell(s(" "), None), "/bin/sh");
    assert_eq!(host_shell(None, s("/p/moor/bin/shell.sh")), "/bin/sh");
}
