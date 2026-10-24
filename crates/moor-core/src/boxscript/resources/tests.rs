use std::process::Command;

use super::{after, parse, PROBE};
use crate::shape::Shape;

#[test]
fn both_lines_give_the_shape() {
    assert_eq!(
        parse("user.name=0\nmoor.cpus=4\nmoor.mem_mib=7900\n"),
        Some(Shape {
            cpus: 4,
            mem_mib: 7900
        })
    );
}

#[test]
fn a_missing_or_empty_value_gives_none() {
    assert_eq!(parse("moor.cpus=4\n"), None);
    assert_eq!(parse("moor.cpus=\nmoor.mem_mib=7900\n"), None);
    assert_eq!(parse("moor.cpus=0\nmoor.mem_mib=7900\n"), None);
    assert_eq!(parse("moor.cpus=4\nmoor.mem_mib=x\n"), None);
    assert_eq!(parse(""), None);
}

#[test]
fn the_probe_run_under_sh_reads_this_machine() {
    let out = Command::new("sh").arg("-c").arg(PROBE).output().unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    let shape = parse(&text);
    assert!(shape.is_some(), "no shape in {text:?}");
    let shape = shape.unwrap();
    let nproc = Command::new("nproc").output().unwrap().stdout;
    let want: u32 = String::from_utf8(nproc).unwrap().trim().parse().unwrap();
    assert_eq!(shape.cpus, want);
    assert!(shape.mem_mib > 0);
}

fn run(script: &str) -> (i32, Option<Shape>) {
    let out = Command::new("sh").arg("-c").arg(script).output().unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    (out.status.code().unwrap(), parse(&text))
}

#[test]
fn the_script_after_keeps_its_exit_code_and_still_probes() {
    for code in [0, 1, 71] {
        let (exit, shape) = run(&after(&format!("(exit {code})")));
        assert_eq!(exit, code);
        assert!(shape.is_some(), "exit {code}");
    }
}

#[test]
fn an_empty_script_after_probes_and_exits_zero() {
    let (exit, shape) = run(&after(""));
    assert_eq!(exit, 0);
    assert!(shape.is_some());
}
