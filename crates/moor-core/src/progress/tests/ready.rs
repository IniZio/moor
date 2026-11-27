//! A Box a person works in by hand, which no agent ever ran in.

use super::*;

#[test]
fn an_idle_box_no_agent_ever_ran_in_is_ready_not_done() {
    let mut b = row(BoxLifecycle::Ready, SessionState::Idle);
    b.by_hand = true;
    let t = derive(&b, &[], ten());
    assert_eq!((t.state, t.line.as_str()), (Progress::Ready, "ready"));
    b.session_state = SessionState::Working;
    assert_eq!(state(&b), Progress::Working);
}
