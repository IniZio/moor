//! `store-sql`: fails when moor writes SQL text, asks which database it runs on, or uses a database driver outside the store's backends.

use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    match store_sql::check(&root) {
        Ok((files, bad)) if bad.is_empty() => {
            println!("store-sql: {files} files ok");
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
