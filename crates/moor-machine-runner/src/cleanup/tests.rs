use serde_json::json;

use super::op;

#[test]
fn the_sweep_names_the_controller_its_machine_keys_answer_carries() {
    let keys = json!({"machines": [], "controller": "1a2b3c4d"});
    let sent = serde_json::to_value(op(vec![7], Some(keys))).unwrap();
    assert_eq!(
        sent,
        json!({"op": "sweep", "keep": [7], "controller": "1a2b3c4d"})
    );
}

#[test]
fn a_controller_that_does_not_say_its_id_sends_an_unscoped_sweep() {
    for keys in [Some(json!({"machines": []})), None] {
        let sent = serde_json::to_value(op(vec![7], keys)).unwrap();
        assert_eq!(sent, json!({"op": "sweep", "keep": [7]}));
    }
}
