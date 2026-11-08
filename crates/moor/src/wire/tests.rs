//! Tests for the composition root.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use super::*;
use crate::moor::Moor;
use moor_core::name::{BoxHandle, BoxPath};
use moor_core::request::SessionsReq;
use moor_core::Request;

fn window(value: Option<&str>) -> Duration {
    let pairs: Vec<(&str, &str)> = value.map(|v| (PROMPT_WINDOW_ENV, v)).into_iter().collect();
    prompt_window(&Env::from_pairs(pairs))
}

#[test]
fn a_built_context_carries_the_prompt_window_from_the_environment() {
    let env = Env::from_pairs([(PROMPT_WINDOW_ENV, "0")]);
    assert_eq!(build(&env).unwrap().prompt_window(), Duration::ZERO);
}

#[test]
fn lookup_deadlines_cover_only_providers_that_set_one() {
    let drivers = BTreeMap::from([
        (
            "external".to_owned(),
            DriverConfig {
                lookup_timeout_secs: std::num::NonZeroU32::new(30),
                ..DriverConfig::default()
            },
        ),
        ("aws".to_owned(), DriverConfig::default()),
    ]);
    assert_eq!(
        lookup_deadlines(&drivers),
        BTreeMap::from([("external".to_owned(), Duration::from_secs(30))])
    );
}

#[test]
fn the_prompt_window_defaults_to_eight_seconds() {
    assert_eq!(window(None), Duration::from_secs(8));
    assert_eq!(window(Some("soon")), Duration::from_secs(8));
}

#[test]
fn the_prompt_window_reads_milliseconds_from_the_environment() {
    assert_eq!(window(Some("250")), Duration::from_millis(250));
    assert_eq!(window(Some(" 0 ")), Duration::ZERO);
}

/// Writes the sh script `body` to `path` as an executable.
fn script(path: &Path, body: &str) {
    moor_testkit::write_executable(path, format!("#!/bin/sh\n{body}\n")).unwrap();
}

/// The shell line of a fake ssh that answers `ssh -G` as crabbox's pinned words do.
const PINNED_G: &str = "test \"$1\" = -G && { echo 'stricthostkeychecking true'; echo 'userknownhostsfile /k'; exit; }";

/// An environment with its state dir at `state`, whose crabbox names one ssh target and whose
/// ssh appends the arguments of each Box call to `<dir>/ssh.log`.
fn fake_tools(dir: &Path, state: &Path) -> Env {
    let crabbox = dir.join("crabbox");
    script(
        &crabbox,
        "case \"$1\" in --version) echo 'crabbox 0.79.1-moor.6';; *) echo \"ssh -o StrictHostKeyChecking=yes 'u@box'\";; esac",
    );
    let ssh = dir.join("ssh");
    let log = dir.join("ssh.log");
    script(
        &ssh,
        &format!("{PINNED_G}\necho \"$@\" >> '{}'\nprintf x", log.display()),
    );
    let root = dir.display().to_string();
    Env::from_pairs([
        ("HOME", root.clone()),
        ("XDG_CONFIG_HOME", root.clone()),
        ("MOOR_CONFIG", format!("{root}/absent.yaml")),
        ("MOOR_STATE_DIR", state.display().to_string()),
        ("MOOR_CRABBOX_BIN", crabbox.display().to_string()),
        ("MOOR_SSH_BIN", ssh.display().to_string()),
    ])
}

#[test]
fn wired_drivers_share_one_short_control_socket_for_a_deep_state_dir() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("d".repeat(120));
    std::fs::create_dir(&state).unwrap();
    let env = fake_tools(dir.path(), &state);
    let handle = BoxHandle::new("external/moor-1-abc123");
    let path = BoxPath::under("/work", "f").unwrap();
    for _ in 0..2 {
        let cx = build(&env).unwrap();
        assert_eq!(cx.take_warnings(), []);
        cx.driver().peek_file(&handle, &path).unwrap();
    }
    let log = std::fs::read_to_string(dir.path().join("ssh.log")).unwrap();
    let sockets: Vec<&str> = log
        .lines()
        .filter_map(|l| {
            l.split_whitespace()
                .find_map(|w| w.strip_prefix("ControlPath="))
        })
        .collect();
    let [first, second] = sockets.as_slice() else {
        return assert_eq!(sockets.len(), 2, "{log}");
    };
    let private = format!("/tmp/moor-{}", rustix::process::getuid().as_raw());
    assert_eq!(Path::new(first).parent(), Some(Path::new(&private)));
    assert_eq!(first, second, "one socket for every freshly wired driver");
}

#[test]
fn a_command_response_says_when_ssh_calls_cannot_share_a_connection() {
    let dir = tempfile::tempdir().unwrap();
    let env = fake_tools(dir.path(), &dir.path().join("state"));
    let sockets = dir.path().join("sockets");
    std::fs::create_dir(&sockets).unwrap();
    std::fs::set_permissions(&sockets, std::fs::Permissions::from_mode(0o755)).unwrap();
    let moor = Moor::from_cx(build_with_sockets(&env, sockets).unwrap());
    let resp = moor.call(Request::Sessions(SessionsReq { parent: None }));
    let codes: Vec<&str> = resp.warnings.iter().map(|w| w.code.as_str()).collect();
    assert_eq!(codes, ["ssh_not_shared"], "{resp:?}");
}

#[test]
fn freshly_wired_drivers_share_one_answer_to_crabbox_ssh_help() {
    let dir = tempfile::tempdir().unwrap();
    let handle = BoxHandle::new("external/moor-1-abc123");
    let path = BoxPath::under("/work", "f").unwrap();
    let helps = dir.path().join("helps");
    for n in 0..2 {
        let env = fake_tools(dir.path(), &dir.path().join(format!("state{n}")));
        script(
            &dir.path().join("crabbox"),
            &format!(
                "test \"$1 $2\" = 'ssh --help' && {{ echo >> '{}'; exit 0; }}\n\
                 case \"$1\" in --version) echo 'crabbox 0.79.1-moor.6';; \
                 *) echo \"ssh -o StrictHostKeyChecking=yes 'u@box'\";; esac",
                helps.display()
            ),
        );
        build(&env)
            .unwrap()
            .driver()
            .peek_file(&handle, &path)
            .unwrap();
    }
    let asked = std::fs::read_to_string(&helps).map_or(0, |s| s.lines().count());
    assert!(
        asked <= 1,
        "two wired drivers, each with its own lookup, asked `crabbox ssh --help` {asked} times"
    );
}

#[test]
fn store_migrate_migrates_and_warns_on_a_store_a_newer_build_migrated() {
    use crate::store::db::testing::open_conn;
    use crate::store::db::DbExt as _;
    let dir = tempfile::tempdir().unwrap();
    let env = Env::from_pairs([("HOME", dir.path().to_str().unwrap())]).with_state_dir(dir.path());
    let line = migrate_store(&env).unwrap();
    assert!(line.starts_with("migrated the store in "), "{line}");
    open_conn(&dir.path().join("moor.db"))
        .run(
            sea_query::Query::insert()
                .into_table("refinery_schema_history")
                .columns(["version", "name", "applied_on", "checksum"])
                .values_panic([
                    10_000.into(),
                    "future".into(),
                    "2030-01-01T00:00:00Z".into(),
                    "1".into(),
                ]),
        )
        .unwrap();
    let line = migrate_store(&env).unwrap();
    assert!(
        line.starts_with("warning: the state database is at migration 10000"),
        "{line}"
    );
}
