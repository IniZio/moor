//! `test-children`: fails when test code holds a spawned child outside a `ChildGuard`.

use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let allow = std::fs::read_to_string(root.join("tools/test-children.allow")).unwrap_or_default();
    match test_children::check(&root, &allow) {
        Ok((files, bad)) if bad.is_empty() => {
            println!("test-children: {files} test files ok");
            ExitCode::SUCCESS
        }
        Ok((_, bad)) => {
            for line in bad {
                eprintln!("{line}");
            }
            eprintln!("{}", test_children::FIX);
            ExitCode::FAILURE
        }
        Err(e) => {
            eprintln!("test-children: {e}");
            ExitCode::FAILURE
        }
    }
}
