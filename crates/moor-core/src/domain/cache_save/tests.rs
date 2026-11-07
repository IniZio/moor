use serde_json::json;

use super::{CacheSave, CacheSaveDenied};
use crate::domain::event::Event;

fn asleep(cache_save: Option<CacheSave>) -> Event {
    Event::Asleep {
        auto: true,
        reason: None,
        cache_save,
    }
}

#[test]
fn an_asleep_event_names_the_decision_and_its_reason() {
    assert_eq!(
        asleep(Some(CacheSave::Allowed)).payload(),
        json!({"auto": true, "cache_save": {"state": "allowed"}})
    );
    let reason = CacheSaveDenied::AgentUnread;
    assert_eq!(
        asleep(Some(CacheSave::Denied { reason })).payload(),
        json!({"auto": true, "cache_save": {"state": "denied", "reason": "agent_unread"}})
    );
}

#[test]
fn every_reason_has_its_own_name() {
    let names = [
        (CacheSaveDenied::AgentNotQuiet, "agent_not_quiet"),
        (CacheSaveDenied::RunInFlight, "run_in_flight"),
        (CacheSaveDenied::AgentUnread, "agent_unread"),
        (CacheSaveDenied::LastResort, "last_resort"),
        (CacheSaveDenied::NoFeature, "no_feature"),
        (CacheSaveDenied::MachineOffline, "machine_offline"),
    ];
    for (reason, name) in names {
        assert_eq!(serde_json::to_value(reason).unwrap(), json!(name));
    }
}

#[test]
fn a_sleep_with_no_decision_writes_no_field_and_an_older_event_still_reads() {
    assert_eq!(asleep(None).payload(), json!({"auto": true}));
    let older: Event = serde_json::from_value(json!({"kind": "asleep", "auto": true})).unwrap();
    assert_eq!(older, asleep(None));
}
