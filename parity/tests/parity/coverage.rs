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
});
