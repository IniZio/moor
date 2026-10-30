//! The env every command run for a Box sees.

use moor_core::config::MachineKindName;

use super::Facts;

/// What a command run for Box `id` in `slot` sees: its id, slot, port offset, machine kind and
/// facts. A Box in its own VM uses the repo's ports, so its offset is 0.
pub(crate) fn box_env(
    id: i64,
    slot: u16,
    kind: MachineKindName,
    facts: &Facts,
) -> Vec<(String, String)> {
    let offset = match kind {
        MachineKindName::Physical => 10 * u32::from(slot),
        MachineKindName::VmHost => 0,
    };
    let mut env = vec![
        ("MOOR_BOX".to_owned(), id.to_string()),
        ("MOOR_SLOT".to_owned(), slot.to_string()),
        ("MOOR_PORT_OFFSET".to_owned(), offset.to_string()),
        ("MOOR_MACHINE_KIND".to_owned(), kind.as_str().to_owned()),
    ];
    env.extend(fact_env(facts));
    env
}

/// `facts` as env: `MOOR_FACT_<KEY>`, the key uppercased with every other character as `_`.
pub(crate) fn fact_env(facts: &Facts) -> Vec<(String, String)> {
    facts
        .iter()
        .map(|(k, v)| {
            let key: String = k
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() {
                        c.to_ascii_uppercase()
                    } else {
                        '_'
                    }
                })
                .collect();
            (format!("MOOR_FACT_{key}"), v.clone())
        })
        .collect()
}
