use super::*;

fn at(lifecycle: BoxLifecycle, hands: HandsState, agent: AgentFact) -> Facts {
    Facts {
        lifecycle,
        hands: Some(hands),
        agent,
        capped: false,
    }
}

#[test]
fn an_awake_box_with_a_ready_agent_is_delivered_to() {
    let f = at(BoxLifecycle::Ready, HandsState::Ready, AgentFact::Ready);
    assert_eq!(next(&f), Next::Deliver);
}

#[test]
fn an_asleep_box_is_woken_and_says_asleep() {
    let f = at(
        BoxLifecycle::Asleep,
        HandsState::Asleep,
        AgentFact::NotReady,
    );
    assert_eq!((next(&f), cause(&f)), (Next::Wake, QueueCause::Asleep));
}

#[test]
fn a_box_waiting_for_room_is_held_for_its_promotion() {
    let f = at(
        BoxLifecycle::Pending,
        HandsState::Asleep,
        AgentFact::NotReady,
    );
    assert_eq!(
        (next(&f), cause(&f)),
        (Next::Hold, QueueCause::WaitingForRoom)
    );
}

#[test]
fn a_box_whose_machine_is_gone_is_brought_back() {
    let f = at(BoxLifecycle::Ready, HandsState::Gone, AgentFact::NotReady);
    assert_eq!(
        (next(&f), cause(&f)),
        (Next::BringBack, QueueCause::Relaunching)
    );
}

#[test]
fn a_dead_agent_is_revived_unless_capped() {
    let mut f = at(BoxLifecycle::Ready, HandsState::Ready, AgentFact::Dead);
    assert_eq!(
        (next(&f), cause(&f)),
        (Next::Revive, QueueCause::Relaunching)
    );
    f.capped = true;
    assert_eq!(
        (next(&f), cause(&f)),
        (Next::Hold, QueueCause::RelaunchCapped)
    );
}

#[test]
fn an_agent_that_cannot_take_a_message_holds_the_queue() {
    let f = at(BoxLifecycle::Ready, HandsState::Ready, AgentFact::NotReady);
    assert_eq!((next(&f), cause(&f)), (Next::Hold, QueueCause::Busy));
}

#[test]
fn a_box_whose_setup_failed_is_held_for_a_retry_even_with_its_machine_gone() {
    let f = at(
        BoxLifecycle::SetupFailed,
        HandsState::Gone,
        AgentFact::NotReady,
    );
    assert_eq!(next(&f), Next::Hold);
}
