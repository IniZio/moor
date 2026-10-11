mod a_agent;
mod a_lifecycle;
mod b_driver;
mod b_run;
mod c_creds;
mod coverage;
mod fake_protocol;
mod harness;

use std::sync::{Arc, Mutex};

use libtest_mimic::{Arguments, Failed, Trial};
use parity::adapter::{self, Moor};
use parity::registry;
use parity::world::World;

pub fn world() -> World {
    World::new(env!("CARGO_BIN_EXE_fake"))
}

pub fn moor() -> Box<dyn Moor> {
    adapter::from_env().unwrap_or_else(|e| panic!("{e}"))
}

fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "panic with non-string payload".to_string()
    }
}

fn main() {
    let args = Arguments::from_args();
    match std::env::var_os("MOOR_BIN") {
        Some(p) if std::path::Path::new(&p).exists() => {}
        _ => {
            eprintln!("parity: MOOR_BIN must be set to an existing moor binary");
            std::process::exit(1);
        }
    }
    let caps = moor().caps();
    let records: Arc<Mutex<Vec<serde_json::Value>>> = Arc::default();
    let trials: Vec<Trial> = registry::cases()
        .into_iter()
        .map(|case| {
            let run = case.run;
            let (id, covers) = (case.id, case.covers);
            let tag = registry::parse_tags(case.tags);
            let label = label_of(&tag);
            let records = Arc::clone(&records);
            let kind = label.clone();
            Trial::test(id, move || {
                let outcome = match &tag {
                    Err(e) => Err(format!("bad tags: {e}")),
                    Ok(_) => std::panic::catch_unwind(run).map_err(panic_message),
                };
                let verdict = match &tag {
                    Ok(t) => registry::verdict(t, caps, outcome),
                    Err(_) => registry::Verdict::Fail(outcome.unwrap_err()),
                };
                records.lock().unwrap().push(serde_json::json!({
                    "id": id,
                    "tag": label,
                    "result": verdict.result(),
                    "covers": covers,
                }));
                match verdict {
                    v if v.ok() => Ok(()),
                    registry::Verdict::Fail(m) => Err(Failed::from(m)),
                    _ => Err(Failed::from(
                        "stale tag: case passes but is tagged new_only",
                    )),
                }
            })
            .with_kind(kind)
        })
        .collect();
    let conclusion = libtest_mimic::run(&args, trials);
    let mut rows = records.lock().unwrap().clone();
    rows.sort_by_key(|r| r["id"].as_str().unwrap_or_default().to_string());
    write_report(&rows);
    conclusion.exit();
}

fn label_of(tag: &Result<registry::Tag, String>) -> String {
    match tag {
        Ok(t) => t.label(),
        Err(_) => "invalid".to_string(),
    }
}

fn write_report(rows: &[serde_json::Value]) {
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| parity::coverage::repo_root().join("target"));
    let dir = target.join("parity");
    std::fs::create_dir_all(&dir).expect("create report dir");
    let body = serde_json::to_string_pretty(rows).expect("serialize report");
    std::fs::write(dir.join("report.json"), body).expect("write report.json");
}
