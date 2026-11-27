//! The Box server's tools, held to the list moor's contract names.

use std::collections::BTreeSet;

use moor_core::contract::TOOLS;

use super::BoxTools;

#[test]
fn the_contract_lists_every_tool_the_box_server_serves() {
    let served: BTreeSet<String> = BoxTools::served()
        .list_all()
        .into_iter()
        .map(|t| t.name.to_string())
        .collect();
    let listed: BTreeSet<String> = TOOLS.iter().map(|(n, _)| (*n).to_owned()).collect();
    assert_eq!(served, listed);
}
