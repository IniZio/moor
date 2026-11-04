//! `store-sql`: fails when the store writes SQL text or asks which database it runs on.

use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let allow = std::fs::read_to_string(root.join("tools/store-sql.allow")).unwrap_or_default();
    match store_sql::check(&root, &allow) {
        Ok((files, bad)) if bad.is_empty() => {
            println!("store-sql: {files} store files ok");
            ExitCode::SUCCESS
        }
        Ok((_, bad)) => {
            for line in bad {
                eprintln!("{line}");
            }
            eprintln!("{}", store_sql::FIX);
            ExitCode::FAILURE
        }
        Err(e) => {
            eprintln!("store-sql: {e}");
            ExitCode::FAILURE
        }
    }
}
