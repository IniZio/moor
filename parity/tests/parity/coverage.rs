use std::collections::BTreeSet;

use parity::coverage;
use parity::parity;
use parity::registry;

parity!(coverage_inventory, covers = [], tags = [], {
    let covered: BTreeSet<String> = registry::cases()
        .iter()
        .flat_map(|c| c.covers.iter().map(|s| s.to_string()))
        .collect();
    let inventory = coverage::parse_inventory(&coverage::inventory_dir()).expect("inventory");
    let exclusions = coverage::load_exclusions(&coverage::exclusions_path()).expect("exclusions");
    match coverage::check(&inventory, &exclusions, &covered) {
        Ok(summary) => {
            let text = summary.render();
            println!("{text}");
            eprintln!("{text}");
        }
        Err(errors) => panic!("{}", errors.join("\n")),
    }
    let smoke = std::fs::read_to_string(coverage::live_smoke_path()).expect("live-smoke.md");
    let live_errors = coverage::check_live_refs(&exclusions, &coverage::parse_live_steps(&smoke));
    assert!(live_errors.is_empty(), "{}", live_errors.join("\n"));
});
