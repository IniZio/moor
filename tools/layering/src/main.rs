//! `layering`: fails when a module uses another its rule table does not allow.

use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let table = std::fs::read_to_string(root.join("tools/layering.tsv")).unwrap_or_default();
    match layering::check(&root, &table) {
        Ok((rules, bad)) if bad.is_empty() => {
            println!("layering: {rules} rules ok");
            ExitCode::SUCCESS
        }
        Ok((_, bad)) => {
            for line in bad {
                eprintln!("{line}");
            }
            ExitCode::FAILURE
        }
        Err(e) => {
            eprintln!("layering: {e}");
            ExitCode::FAILURE
        }
    }
}
