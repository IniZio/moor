//! The Box server's tools, held to the list moor's contract names.

use std::collections::BTreeSet;

use moor_core::contract::TOOLS;

use super::BoxTools;

/// Tools the router serves that no launch's contract lists yet: the reader's, until its slice.
const NOT_YET_IN_THE_CONTRACT: &[&str] = &["hand_in_reading"];

#[test]
fn the_contract_lists_every_tool_the_routers_serve() {
    let served: BTreeSet<String> = (BoxTools::tool_router() + BoxTools::place_router())
        .list_all()
        .into_iter()
        .map(|t| t.name.to_string())
        .filter(|n| !NOT_YET_IN_THE_CONTRACT.contains(&n.as_str()))
        .collect();
    let listed: BTreeSet<String> = TOOLS.iter().map(|(n, _)| (*n).to_owned()).collect();
    assert_eq!(served, listed);
}
