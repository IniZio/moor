//! Every ssh moor runs to a Box keeps crabbox's host-key pin, or does not run.

use std::fs;
use std::time::Duration;

use moor_core::name::BoxPath;
use moor_core::seam::driver::{BoxStep, Driver, DriverError, ExecSpec};
use moor_core::seam::proc::Deadline;

use super::fake_ssh::{handle, lab, Lab};

const UNPINNED: [&str; 6] = [
    "ssh -o 'StrictHostKeyChecking=off' -o 'StrictHostKeyChecking=yes' 'u@box'",
    "ssh -o 'StrictHostKeyChecking=no' 'u@box'",
    "ssh -vo 'StrictHostKeyChecking=no' -o 'StrictHostKeyChecking=yes' 'u@box'",
    "ssh -o 'StrictHostKeyChecking=yes' -o 'UserKnownHostsFile=/dev/null' 'u@box'",
    "ssh -o 'StrictHostKeyChecking=yes' -o 'UserKnownHostsFile=/dev/./null' 'u@box'",
    "ssh 'u@box'",
];

fn deadline() -> Deadline {
    Deadline(Duration::from_secs(20))
}

/// The result of each kind of Box ssh moor runs, by name.
fn every_box_ssh(lab: &Lab) -> Vec<(&'static str, Result<(), DriverError>)> {
    let h = handle();
    let path = BoxPath::restore("/w/a").unwrap();
    let spec = ExecSpec {
        argv: vec!["true".to_owned()],
        env: Vec::new(),
        cwd: None,
        output: None,
    };
    let batch = |connect: &[String]| {
        let done = lab
            .cb
            .batch(&h, connect, &[BoxStep::Peek(path.clone())], deadline());
        done.into_iter().next().unwrap().map(|_| ())
    };
    let pane = lab
        .cb
        .connect_argv(&h)
        .unwrap_or_else(|_| ["ssh", "-t", "u@box"].map(str::to_owned).to_vec());
    let mut sink = |_: &str, _: Option<&mut dyn std::io::Read>| {};
    vec![
        ("exec", lab.cb.exec(&h, &spec, deadline()).map(|_| ())),
        ("batch", batch(&[])),
        ("batch through a pane's argv", batch(&pane)),
        ("peek", lab.cb.peek_file(&h, &path).map(|_| ())),
        ("put", lab.cb.put_file(&h, &path, b"x", 0o600)),
        (
            "stream",
            lab.cb
                .stream_files(&h, &path, &["a".to_owned()], deadline(), &mut sink),
        ),
        ("connect", lab.cb.connect_argv(&h).map(|_| ())),
        (
            "copy",
            lab.cb.copy_out(&h, &path, &lab.dir.path().join("copied")),
        ),
    ]
}

#[test]
fn every_box_ssh_refuses_words_that_do_not_pin_the_host_key_and_runs_no_ssh() {
    for words in UNPINNED {
        let lab = lab();
        fs::write(lab.bin().join("words"), format!("{words}\n")).unwrap();
        for (call, got) in every_box_ssh(&lab) {
            assert!(
                matches!(&got, Err(DriverError::Failed { summary }) if summary.contains("host key")),
                "{call} with {words}: {got:?}"
            );
        }
        assert!(
            lab.argvs("ssh").is_empty(),
            "{words}: {:?}",
            lab.argvs("ssh")
        );
        assert!(!lab.argvs("opts").is_empty(), "{words}: ssh -G was asked");
    }
}

#[test]
fn every_box_ssh_runs_with_crabbox_pinned_words() {
    let lab = lab();
    fs::create_dir_all(lab.dir.path().join("w")).unwrap();
    for (call, got) in every_box_ssh(&lab) {
        assert!(
            !matches!(&got, Err(DriverError::Failed { summary }) if summary.contains("host key")),
            "{call}: {got:?}"
        );
    }
    let ssh = lab.argvs("ssh");
    assert!(ssh.len() >= 6, "{ssh:?}");
    for argv in &ssh {
        lab.assert_on_the_master_and_pinned(argv);
    }
}
