mod a_lifecycle;
mod fake_protocol;
mod harness;

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
    let trials: Vec<Trial> = registry::cases()
        .into_iter()
        .map(|case| {
            let run = case.run;
            Trial::test(case.id, move || {
                std::panic::catch_unwind(run).map_err(|p| Failed::from(panic_message(p)))
            })
            .with_kind(case.tags.join(","))
        })
        .collect();
    libtest_mimic::run(&args, trials).exit();
}
