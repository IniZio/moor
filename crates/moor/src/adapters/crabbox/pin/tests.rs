//! The check asks the host's real ssh, with `ssh -G`, which opens no connection.

use std::fs;
use std::os::unix::fs::symlink;
use std::path::PathBuf;
use std::sync::Arc;

use moor_core::seam::clock::Ts;
use moor_core::seam::driver::DriverError;
use tempfile::TempDir;

use super::check;
use crate::adapters::crabbox::Crabbox;
use crate::adapters::proc::{SystemProc, ToolPaths};
use crate::env::Env;
use crate::testing::clock::FakeClock;
use crate::testing::rng::FakeRng;

/// A driver whose `ssh -G` reads run the host's ssh, and a dir for config files.
struct Real {
    dir: TempDir,
    cb: Crabbox,
}

fn real_with(tools: &ToolPaths) -> Real {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("empty"), "").unwrap();
    let proc = Arc::new(SystemProc::new(
        &Env::from_pairs([("PATH", "/usr/bin:/bin")]),
        tools,
    ));
    let clock = Arc::new(FakeClock::new(Ts::from_millis(0)));
    let cb = Crabbox::new(proc, clock, Arc::new(FakeRng::default()));
    Real { dir, cb }
}

fn real() -> Real {
    real_with(&ToolPaths::default())
}

impl Real {
    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    /// `line` split on spaces, after an empty `-F` config that keeps this host's ssh config
    /// out; a later `-F` in `line` replaces it, as ssh keeps the last.
    fn words(&self, line: &str) -> Vec<String> {
        let empty = self.path("empty").display().to_string();
        ["-F".to_owned(), empty]
            .into_iter()
            .chain(line.split(' ').map(str::to_owned))
            .collect()
    }

    fn passes(&self, line: &str) {
        assert_eq!(check(&self.cb, &self.words(line)), Ok(()), "{line}");
    }

    fn refuses(&self, words: &[String], why: &str) {
        match check(&self.cb, words) {
            Err(DriverError::Failed { summary }) => {
                assert!(summary.contains(why), "{words:?}: {summary}");
                assert!(summary.contains("host key"), "{words:?}: {summary}");
            }
            other => unreachable!("{words:?}: expected a refusal, got {other:?}"),
        }
    }
}

const STRICT: &str = "turn host key checking off";
const FILE: &str = "UserKnownHostsFile";

#[test]
fn crabbox_pinned_words_pass_in_every_spelling() {
    let r = real();
    for line in [
        "-o StrictHostKeyChecking=yes -o UserKnownHostsFile=/lease/known_hosts u@box",
        "-o ControlMaster=no -o ControlPath=none -o StrictHostKeyChecking=yes -o UserKnownHostsFile=/lease/known_hosts -o GlobalKnownHostsFile=none -o HostKeyAlias=crabbox-lease-1f2e u@box",
        "-oStrictHostKeyChecking=yes -oUserKnownHostsFile=/lease/known_hosts u@box",
        "-o stricthostkeychecking=YES -o UserKnownHostsFile=/lease/known_hosts u@box",
        "-i /k/id -o StrictHostKeyChecking=yes -o StrictHostKeyChecking=no -o UserKnownHostsFile=/lease/known_hosts u@box",
        "-o StrictHostKeyChecking=yes -o UserKnownHostsFile=/lease/known_hosts -o UserKnownHostsFile=/dev/null u@box",
        "-o StrictHostKeyChecking=accept-new -o UserKnownHostsFile=/lease/known_hosts u@box",
        "-o StrictHostKeyChecking=accept-new -o UserKnownHostsFile=/lease/known_hosts -o StrictHostKeyChecking=yes u@box",
    ] {
        r.passes(line);
    }
    let spaced = r
        .words("u@box")
        .into_iter()
        .chain(
            [
                "-o",
                "StrictHostKeyChecking yes",
                "-o",
                "UserKnownHostsFile /lease/kh",
            ]
            .map(str::to_owned),
        )
        .collect::<Vec<_>>();
    assert_eq!(check(&r.cb, &spaced), Ok(()));
}

#[test]
fn words_that_turn_host_key_checking_off_are_refused() {
    let r = real();
    for line in [
        "u@box",
        "-p 22 u@box",
        "-o StrictHostKeyChecking=off u@box",
        "-o StrictHostKeyChecking=false u@box",
        "-o StrictHostKeyChecking=no -o StrictHostKeyChecking=accept-new u@box",
        "-oStrictHostKeyChecking=no -o StrictHostKeyChecking=yes u@box",
        "-o StrictHostKeyChecking=ask u@box",
        "-vo StrictHostKeyChecking=no -o StrictHostKeyChecking=yes u@box",
        "-Nvo StrictHostKeyChecking=no -o StrictHostKeyChecking=yes u@box",
        "u@box -o StrictHostKeyChecking=no",
        "u@box true -oStrictHostKeyChecking=yes",
    ] {
        r.refuses(&r.words(line), STRICT);
    }
    let tabbed = r
        .words("u@box")
        .into_iter()
        .chain(
            [
                "-o",
                "StrictHostKeyChecking\tno",
                "-o",
                "StrictHostKeyChecking=yes",
            ]
            .map(str::to_owned),
        )
        .collect::<Vec<_>>();
    r.refuses(&tabbed, STRICT);
}

#[test]
fn a_known_hosts_file_that_holds_no_keys_is_refused() {
    let r = real();
    symlink("/dev/null", r.path("null-link")).unwrap();
    let linked = format!("UserKnownHostsFile={}", r.path("null-link").display());
    for file in [
        "UserKnownHostsFile=/dev/null",
        "UserKnownHostsFile=\"/dev/null\"",
        "UserKnownHostsFile=/dev/./null",
        "UserKnownHostsFile=//dev//null",
        "UserKnownHostsFile=none",
        "UserKnownHostsFile=/dev/zero",
        &linked,
    ] {
        let line = format!("-o StrictHostKeyChecking=yes -o {file} u@box");
        r.refuses(&r.words(&line), FILE);
    }
    r.refuses(
        &r.words("-o StrictHostKeyChecking=accept-new -o UserKnownHostsFile=/dev/null u@box"),
        FILE,
    );
    let listed = r
        .words("-o StrictHostKeyChecking=yes u@box")
        .into_iter()
        .chain(["-o", "UserKnownHostsFile /lease/kh /dev/null"].map(str::to_owned))
        .collect::<Vec<_>>();
    r.refuses(&listed, FILE);
    let tabbed = r
        .words("-o StrictHostKeyChecking=yes u@box")
        .into_iter()
        .chain(["-o", "UserKnownHostsFile\t/dev/null"].map(str::to_owned))
        .collect::<Vec<_>>();
    r.refuses(&tabbed, FILE);
}

#[test]
fn a_config_file_that_drops_the_known_hosts_is_refused() {
    let r = real();
    let config = r.path("config");
    fs::write(&config, "Host *\n  UserKnownHostsFile /dev/null\n").unwrap();
    let line = format!("-o StrictHostKeyChecking=yes -F {} u@box", config.display());
    r.refuses(&r.words(&line), FILE);
    fs::write(&config, "Host *\n  StrictHostKeyChecking no\n").unwrap();
    let line = format!(
        "-F {} -o UserKnownHostsFile=/lease/kh u@box",
        config.display()
    );
    r.refuses(&r.words(&line), STRICT);
}

#[test]
fn words_ssh_cannot_read_are_refused_and_a_missing_ssh_is_unavailable() {
    let r = real();
    for line in [
        "-o StrictHostKeyChecking=bogus -o UserKnownHostsFile=/lease/kh u@box",
        "-o UserKnownHostsFile= -o StrictHostKeyChecking=yes u@box",
    ] {
        r.refuses(&r.words(line), "ssh -G could not read them");
    }
    let gone = real_with(&ToolPaths {
        ssh: Some(PathBuf::from("/nonexistent/ssh")),
        ..ToolPaths::default()
    });
    let line = "-o StrictHostKeyChecking=yes -o UserKnownHostsFile=/lease/kh u@box";
    assert_eq!(
        check(&gone.cb, &gone.words(line)),
        Err(DriverError::Unavailable(
            "ssh could not be started".to_owned()
        ))
    );
}

#[test]
fn an_answer_without_a_known_hosts_file_is_refused() {
    use crate::testing::proc::{Reply, ScriptedProc};
    use moor_core::seam::proc::Tool;
    for answer in [
        "stricthostkeychecking true\n",
        "stricthostkeychecking true\nuserknownhostsfile \n",
    ] {
        let mut r = real();
        let options = ScriptedProc::new();
        options.on(Tool::Ssh, &["-G"], Reply::Ok(answer.to_owned()));
        r.cb.options = Arc::new(options);
        r.refuses(&r.words("u@box"), FILE);
    }
}
