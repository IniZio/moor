use serde_json::json;

use super::queued_words;

#[test]
fn a_cause_reads_as_words_with_the_resource_waited_for_when_known() {
    let words = |v| queued_words(&v);
    assert_eq!(words(json!({"cause": "asleep"})), "asleep");
    assert_eq!(
        words(json!({"cause": "waiting_for_room"})),
        "waiting for room"
    );
    assert_eq!(
        words(json!({"cause": "waiting_for_room", "resource": "memory"})),
        "waiting for room: memory"
    );
    assert_eq!(words(json!({})), "busy");
}
