//! Tests for the composition root.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use super::*;
use crate::moor::Moor;
use crate::testing::clock::FakeClock;
use crate::testing::proc::{Reply, ScriptedProc};
use moor_core::activity::PaneStatus;
use moor_core::name::{BoxHandle, BoxName, BoxPath};
use moor_core::request::SessionsReq;
use moor_core::seam::clock::Ts;
use moor_core::seam::proc::Tool;
use moor_core::seam::surface::{PaneRef, ResumeReg};
use moor_core::Request;

fn window(value: Option<&str>) -> Duration {
    let pairs: Vec<(&str, &str)> = value.map(|v| (PROMPT_WINDOW_ENV, v)).into_iter().collect();
    prompt_window(&Env::from_pairs(pairs))
}

#[test]
fn the_herdr_adapter_numbers_its_reports_from_the_clock() {
    let proc = Arc::new(ScriptedProc::new());
    proc.on(Tool::Herdr, &["pane"], Reply::Ok(String::new()));
    let clock = Arc::new(FakeClock::new(Ts::from_millis(7_000)));
    let h = herdr(
        &(proc.clone() as Arc<dyn Proc>),
        "s",
        Vec::new(),
        clock,
        None,
    );
    let reg = ResumeReg {
        agent: "claude".to_owned(),
        state: PaneStatus::Idle,
        session_id: "abc".to_owned(),
        box_name: BoxName::parse_allow_bare("a").unwrap(),
    };
    h.register_resume(&PaneRef::new("p"), &reg).unwrap();
    let calls = proc.calls();
    assert_eq!(calls.len(), 2);
    for c in calls {
        let i = c.args.iter().position(|a| a == "--seq").unwrap();
        let seq: u64 = c.args.get(i + 1).unwrap().parse().unwrap();
        assert!(seq >= 7_000_000, "{seq}");
    }
}

#[test]
fn a_built_context_carries_the_prompt_window_from_the_environment() {
    let env = Env::from_pairs([(PROMPT_WINDOW_ENV, "0")]);
    assert_eq!(build(&env).unwrap().prompt_window(), Duration::ZERO);
}

/// A writer whose bytes the test can read back after the proxy owns it.
struct Shared(Arc<Mutex<Vec<u8>>>);

impl Write for Shared {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn serving_an_unknown_box_fails_with_one_stderr_line() {
    let home = tempfile::tempdir().unwrap();
    let root = home.path().to_str().unwrap();
    let state = home.path().join("state");
    let env = Env::from_pairs([
        ("HOME", root),
        ("XDG_CONFIG_HOME", root),
        ("XDG_STATE_HOME", root),
        ("MOOR_CONFIG", &format!("{root}/absent.yaml")),
        ("MOOR_STATE_DIR", state.to_str().unwrap()),
    ]);
    let out = Arc::new(Mutex::new(Vec::new()));
    let code = serve_proxy(&env, BoxId::new(1), Shared(Arc::clone(&out)));
    assert_ne!(code, 0);
    let text = String::from_utf8(out.lock().unwrap().clone()).unwrap();
    assert!(text.starts_with("moor proxy: "), "{text}");
    assert_eq!(text.lines().count(), 1, "{text}");
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

fn kind_of(config: Option<SurfaceKind>, env: &[(&str, &str)]) -> Result<SurfaceKind, CommandError> {
    let mut c = UserConfig::default();
    c.surface.kind = config;
    surface_kind(&c, &Env::from_pairs(env.iter().copied()))
}

#[test]
fn the_surface_is_herdr_when_neither_config_nor_environment_names_one() {
    assert_eq!(kind_of(None, &[]).unwrap(), SurfaceKind::Herdr);
    assert_eq!(
        kind_of(None, &[(SURFACE_ENV, " ")]).unwrap(),
        SurfaceKind::Herdr
    );
}

#[test]
fn the_environment_names_the_surface_when_the_config_does_not() {
    assert_eq!(
        kind_of(None, &[(SURFACE_ENV, "pty")]).unwrap(),
        SurfaceKind::Pty
    );
    assert_eq!(
        kind_of(None, &[(SURFACE_ENV, "herdr")]).unwrap(),
        SurfaceKind::Herdr
    );
}

#[test]
fn the_config_surface_wins_over_the_environment() {
    assert_eq!(
        kind_of(Some(SurfaceKind::Herdr), &[(SURFACE_ENV, "pty")]).unwrap(),
        SurfaceKind::Herdr
    );
}

#[test]
fn an_unknown_surface_in_the_environment_is_a_usage_error() {
    let e = kind_of(None, &[(SURFACE_ENV, "tmux")]).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Usage);
    assert!(e.error.to_string().contains("MOOR_SURFACE"), "{}", e.error);
}

#[cfg(not(feature = "pty-surface"))]
#[test]
fn a_build_without_the_pty_feature_refuses_the_pty_surface() {
    let e = build(&Env::from_pairs([(SURFACE_ENV, "pty")]))
        .err()
        .unwrap();
    assert_eq!(e.kind, ErrorKind::Usage);
    assert!(e.error.to_string().contains("pty-surface"), "{}", e.error);
}

/// A herdr that answers every call with `server_not_running`, and what it was asked.
fn herdr_without_server() -> (Arc<ScriptedProc>, Arc<dyn Proc>) {
    let proc = Arc::new(ScriptedProc::new());
    proc.on(
        Tool::Herdr,
        &[],
        Reply::Fail {
            code: 1,
            stdout: String::new(),
            stderr:
                r#"{"error":{"code":"server_not_running","message":"no herdr server is running"}}"#
                    .to_owned(),
        },
    );
    let dyn_proc = proc.clone() as Arc<dyn Proc>;
    (proc, dyn_proc)
}

/// The failure MCP `send_to_box` hit: the default herdr surface reads the pane through a herdr
/// server, and none runs on the controller.
#[test]
fn the_herdr_surface_cannot_read_a_pane_with_no_herdr_server() {
    let (_, proc) = herdr_without_server();
    let clock: Arc<dyn Clock> = Arc::new(FakeClock::new(Ts::from_millis(0)));
    let config = UserConfig::default();
    let kind = surface_kind(&config, &Env::from_pairs(Vec::<(&str, &str)>::new())).unwrap();
    let s = surface(kind, &config, &proc, Vec::new(), clock, None).unwrap();
    let e = s
        .pane_obs(
            &PaneRef::new("p"),
            moor_core::seam::proc::Deadline(Duration::from_secs(1)),
        )
        .unwrap_err();
    assert!(e.to_string().contains("herdr server not running"), "{e}");
}

/// The surface `MOOR_SURFACE=pty` wires types into and reads its pane while herdr has no
/// server, and never runs herdr.
#[cfg(feature = "pty-surface")]
#[test]
fn the_pty_surface_types_into_a_pane_with_no_herdr_server() {
    use moor_core::seam::proc::Deadline;
    use moor_core::seam::surface::PaneCmd;

    let (calls, proc) = herdr_without_server();
    let clock: Arc<dyn Clock> = Arc::new(FakeClock::new(Ts::from_millis(0)));
    let config = UserConfig::default();
    let kind = surface_kind(&config, &Env::from_pairs([(SURFACE_ENV, "pty")])).unwrap();
    assert_eq!(kind, SurfaceKind::Pty);
    let s = surface(kind, &config, &proc, Vec::new(), clock, None).unwrap();
    let ws = s.ensure_workspace("moor:no-herdr").unwrap();
    let pane = s
        .open_pane(
            &ws,
            &PaneCmd {
                label: "agent".to_owned(),
                argv: vec!["sh".to_owned(), "-c".to_owned(), "exec cat".to_owned()],
                env: Vec::new(),
            },
        )
        .unwrap();
    s.send_text(&pane, "hello from mcp\r").unwrap();
    let deadline = Deadline(Duration::from_secs(5));
    let mut tail = String::new();
    for _ in 0..500 {
        tail = s.pane_obs(&pane, deadline).unwrap().tail;
        if tail.matches("hello from mcp").count() >= 2 {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(tail.matches("hello from mcp").count(), 2, "{tail}");
    s.close_pane(&pane).unwrap();
    assert!(calls.calls().iter().all(|c| c.tool != Tool::Herdr));
}
