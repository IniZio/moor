//! Fill the effect in flight and the wanted close of each open Box from the intent the binary
//! before this one wrote, as the first binary to read them starts, then clear every intent. A
//! closed or rejected Box has nothing in flight, so an intent left on one is dropped.

use moor_migrate::Script;

/// Each intent kind with the effect it stands for.
const EFFECTS: [(&str, &str); 4] = [
    ("provisioning", "acquire"),
    ("setup", "setup"),
    ("launching", "launch"),
    ("running", "run"),
];

/// Write the effect, its attempt and its run, and the wanted close, of every open Box.
pub(super) fn write(s: &mut Script) {
    let open = "\"state\" NOT IN ('closed', 'rejected') AND \"effect\" IS NULL \
                AND \"wanted\" IS NULL";
    let arms = EFFECTS
        .iter()
        .map(|(intent, effect)| format!("WHEN '{intent}' THEN '{effect}' "))
        .collect::<Vec<_>>()
        .concat();
    let kinds = EFFECTS
        .iter()
        .map(|(intent, _)| format!("'{intent}'"))
        .collect::<Vec<_>>()
        .join(", ");
    s.raw(&format!(
        "UPDATE \"box_state\" SET \"effect\" = CASE \"intent_kind\" {arms}END, \
         \"effect_attempt\" = 1, \
         \"effect_run\" = CASE WHEN \"intent_kind\" = 'running' THEN \"intent_run\" END, \
         \"effect_at\" = \"intent_at\" \
         WHERE {open} AND \"intent_kind\" IN ({kinds}) \
         AND (\"intent_kind\" <> 'running' OR \"intent_run\" IS NOT NULL)"
    ));
    // A cut acquire that went again asked for a machine a second time since its start began.
    let of = "\"box_state\".\"box\"";
    let began = format!(
        "COALESCE((SELECT MAX(\"e\".\"id\") FROM \"events\" AS \"e\" \
         WHERE \"e\".\"box\" = {of} AND \"e\".\"kind\" = 'setting_up'), 0)"
    );
    let asked = format!(
        "(SELECT COUNT(*) FROM \"events\" AS \"e\" WHERE \"e\".\"box\" = {of} \
         AND \"e\".\"kind\" = 'box_requested' AND \"e\".\"id\" > {began})"
    );
    s.raw(&format!(
        "UPDATE \"box_state\" SET \"effect_attempt\" = 2 \
         WHERE \"effect\" = 'acquire' AND {asked} > 1"
    ));
    s.raw(&format!(
        "UPDATE \"box_state\" SET \"wanted\" = 'close', \"wanted_force\" = 1, \
         \"wanted_at\" = \"intent_at\" WHERE {open} AND \"intent_kind\" = 'closing'"
    ));
    // A binary that still reads intents, rolled back to, must find none left from before.
    s.raw(
        "UPDATE \"box_state\" SET \"intent_kind\" = NULL, \"intent_pid\" = NULL, \
         \"intent_run\" = NULL, \"intent_at\" = NULL WHERE \"intent_kind\" IS NOT NULL",
    );
}
