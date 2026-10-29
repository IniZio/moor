use std::time::Duration;

use moor_core::seam::driver::DriverError;

use super::Watch;

const RETRYING: &str = "\
leased lease-7f3a slug=box-a server=12 type=small ip=10.0.0.9 via coordinator
waiting for 10.0.0.9:22 external provider SSH... elapsed=0s remaining=20m0s ports=22:closed
waiting for 10.0.0.9:22 external provider SSH ssh-auth... elapsed=10s remaining=19m50s ports=22:auth
waiting for 10.0.0.9:22 external provider SSH ssh-auth... elapsed=20s remaining=19m40s ports=22:auth
waiting for 10.0.0.9:22 external provider SSH ssh-auth... elapsed=30s remaining=19m30s ports=22:auth
";

fn notes(out: &str) -> (Watch, Vec<String>) {
    let mut watch = Watch::default();
    let mut got = Vec::new();
    for line in out.lines() {
        watch.see(line, &mut |n| got.push(n.to_owned()));
    }
    (watch, got)
}

#[test]
fn a_retrying_ssh_wait_reports_each_distinct_step_once() {
    let (_, got) = notes(RETRYING);
    assert_eq!(
        got,
        [
            "machine created",
            "waiting for SSH: port 22 closed",
            "waiting for SSH: login refused on port 22",
        ]
    );
}

#[test]
fn a_timeout_names_what_the_acquire_waited_on() {
    let (watch, _) = notes(RETRYING);
    let err = watch.explain(DriverError::Timeout(Duration::from_mins(15)));
    assert_eq!(
        err.to_string(),
        "timed out after 900s waiting for SSH: login refused on port 22"
    );
}

#[test]
fn an_ssh_client_error_is_reported_in_fixed_words() {
    let (watch, got) = notes("user@10.0.0.9: Permission denied (publickey,password).\n");
    assert_eq!(
        got,
        ["waiting for SSH: Permission denied (publickey,password)"]
    );
    let err = watch.explain(DriverError::Timeout(Duration::from_mins(15)));
    assert_eq!(
        err.to_string(),
        "timed out after 900s waiting for SSH: Permission denied (publickey,password)"
    );
}

#[test]
fn a_success_reports_its_steps_and_leaves_other_errors_alone() {
    let (watch, got) = notes(
        "leased l1 slug=s server=1 type=t ip=10.0.0.1 via coordinator\n\
         using ssh port 2222 for 10.0.0.1 (configured 22 not ready)\n",
    );
    assert_eq!(got, ["machine created", "SSH answered on port 2222"]);
    assert_eq!(watch.explain(DriverError::NotFound), DriverError::NotFound);
}

#[test]
fn lines_of_unknown_shape_never_pass_through() {
    let secret = "\
GITHUB_TOKEN=ghp_abcdefghijklmnopqrstuvwxyz0123456789
-----BEGIN OPENSSH PRIVATE KEY-----
b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW
Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.e30.abc
Permission denied (ghp_abc123TOKEN)
";
    let (watch, got) = notes(secret);
    assert!(got.is_empty(), "{got:?}");
    let err = watch.explain(DriverError::Timeout(Duration::from_mins(15)));
    assert_eq!(err, DriverError::Timeout(Duration::from_mins(15)));
}

#[test]
fn hosts_and_lease_ids_are_not_forwarded() {
    let (_, got) = notes(RETRYING);
    assert!(got
        .iter()
        .all(|n| !n.contains("10.0.0.9") && !n.contains("lease-7f3a")));
}
