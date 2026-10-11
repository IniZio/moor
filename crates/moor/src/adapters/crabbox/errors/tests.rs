use std::io;
use std::time::Duration;

use moor_core::seam::driver::DriverError;
use moor_core::seam::proc::{ProcError, Tool};

use super::{map_error, Op};

fn nz(code: i32, stderr: &str) -> ProcError {
    ProcError::NonZero {
        code,
        stdout: String::new(),
        stderr_tail: stderr.to_owned(),
    }
}

fn failed(s: &str) -> DriverError {
    DriverError::Failed {
        summary: s.to_owned(),
    }
}

#[test]
fn exit_and_text_table() {
    let cases = [
        (nz(4, "box cbx_1 not found\n"), DriverError::NotFound),
        (nz(4, "Not Found"), DriverError::NotFound),
        (
            nz(2, "quota exceeded for plan"),
            DriverError::Quota("the plan limit was reached".to_owned()),
        ),
        (
            nz(1, "401 Unauthorized"),
            DriverError::Auth("the provider rejected the credentials".to_owned()),
        ),
        (
            nz(4, "at capacity"),
            DriverError::Capacity {
                limit: None,
                holder: None,
            },
        ),
        (
            nz(3, "bad config"),
            failed("box release failed (crabbox configuration problem)"),
        ),
        (
            nz(5, "lost it"),
            failed("box release failed (box lost during warmup)"),
        ),
        (
            nz(1, "release failed"),
            failed("box release failed (crabbox exit 1)"),
        ),
        (
            nz(4, "refused"),
            failed("box release failed (crabbox exit 4)"),
        ),
        (
            nz(2, "usage: x"),
            failed("box release failed (crabbox exit 2)"),
        ),
        (
            ProcError::Signalled(9),
            failed("box release failed (crabbox was killed)"),
        ),
        (
            ProcError::Spawn(io::Error::other("nope")),
            DriverError::Unavailable("crabbox could not be started".to_owned()),
        ),
        (
            ProcError::Timeout {
                tool: Tool::Crabbox,
                after: Duration::from_secs(7),
            },
            DriverError::Timeout(Duration::from_secs(7)),
        ),
    ];
    for (input, want) in cases {
        assert_eq!(map_error(Op::Release, None, &input), want, "{input:?}");
    }
}

#[test]
fn op_names_the_summary() {
    let e = nz(1, "x");
    assert_eq!(
        map_error(Op::Acquire, None, &e),
        failed("box acquire failed (crabbox exit 1)")
    );
    assert_eq!(
        map_error(Op::List, None, &e),
        failed("box list failed (crabbox exit 1)")
    );
    assert_eq!(
        map_error(Op::Workdir, None, &e),
        failed("box workdir lookup failed (crabbox exit 1)")
    );
    assert_eq!(
        map_error(Op::Caps, None, &e),
        failed("provider lookup failed (crabbox exit 1)")
    );
}

#[test]
fn provider_named_in_exit_summary() {
    assert_eq!(
        map_error(Op::Release, Some("local-container"), &nz(1, "x")),
        failed("box release failed on provider local-container (exit 1)")
    );
    assert_eq!(
        map_error(Op::Acquire, Some("aws"), &nz(1, "x")),
        failed("box acquire failed on provider aws (exit 1)")
    );
}

#[test]
fn raw_stderr_never_leaks() {
    let token = "SECRETTOKEN123";
    let texts = [
        format!("quota exceeded {token}"),
        format!("unauthorized {token}"),
        format!("capacity {token}"),
        format!("not found {token}"),
        format!("boom {token}"),
    ];
    for code in [1, 2, 3, 4, 5] {
        for t in &texts {
            let out = format!(
                "{:?} {}",
                map_error(Op::Acquire, None, &nz(code, t)),
                map_error(Op::Acquire, None, &nz(code, t))
            );
            assert!(!out.contains(token), "{code} {t}: {out}");
        }
    }
}
