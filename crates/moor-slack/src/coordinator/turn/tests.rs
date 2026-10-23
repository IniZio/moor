//! Tests for one coordinator turn against the fake `claude` script, which records what it got
//! and answers by the lease it found in `HTTPS_PROXY`.
#![allow(clippy::panic, reason = "test assertions")]

use std::path::{Path, PathBuf};
use std::time::Duration;

use moor_core::secret::Secret;

use super::{run, Ended, Limits, Turn, LIMITS};
use crate::coordinator::claude::tests::install;
use crate::coordinator::claude::{Claude, HostEnv};
use crate::coordinator::{Answer, ProxyAccess, ToolServer, TurnSetup};

/// A probed fake in `bin`.
fn claude(bin: &Path) -> Claude {
    let host = HostEnv {
        home: Some("/home/op".into()),
        user: Some("op".into()),
    };
    Claude::probe(&install(bin, None), host).unwrap()
}

/// Dirs a turn runs with: the fake's records, the config dir and the workdir.
struct Dirs {
    bin: tempfile::TempDir,
    config: tempfile::TempDir,
    work: tempfile::TempDir,
}

impl Dirs {
    fn new() -> Self {
        Self {
            bin: tempfile::tempdir().unwrap(),
            config: tempfile::tempdir().unwrap(),
            work: tempfile::tempdir().unwrap(),
        }
    }

    /// Runs one turn on lease `token` within `limits`; returns how it ended and its pid.
    fn turn(&self, token: &str, limits: Limits) -> (Ended, u32) {
        let claude = claude(self.bin.path());
        let token = Secret::new(token.into());
        let proxy = ProxyAccess {
            port: 4433,
            ca: Path::new("/srv/moor/proxy-ca.pem"),
            lease: &token,
        };
        let args = ["mcp".to_owned(), "--thread".to_owned(), "C1/1.0".to_owned()];
        let server = ToolServer {
            name: "moor",
            program: Path::new("/opt/moor/moor-slack"),
            args: &args,
            tools: &["list_boxes", "close_box"],
        };
        let turn = Turn {
            setup: TurnSetup {
                model: "sonnet",
                config_dir: self.config.path(),
                workdir: self.work.path(),
                brief: "Be brief.",
                tools: &server,
                proxy: &proxy,
            },
            input: "## New\nhello",
            limits,
        };
        let mut pid = 0;
        let ended = run(&claude, &turn, &mut |p| pid = p, &|_| {});
        (ended, pid)
    }

    fn recorded(&self, file: &str) -> String {
        std::fs::read_to_string(self.bin.path().join("run-0").join(file)).unwrap()
    }
}

/// Limits short enough for a test.
const SHORT: Limits = Limits {
    start: Duration::from_secs(5),
    turn: Duration::from_millis(700),
};

#[test]
fn a_turn_gets_its_argv_stdin_and_the_proxy_in_its_env_and_no_token() {
    let dirs = Dirs::new();
    let token = "lease-good-51d0";
    let (ended, _) = dirs.turn(token, LIMITS);
    let want = Answer::Reply {
        text: "OK".to_owned(),
        cost_usd: Some(0.0125),
    };
    assert_eq!(ended, Ended::Answered(want));
    let fds = dirs.recorded("fds");
    assert!(
        !fds.lines().any(|l| l.contains(" 3 -> ")),
        "no pipe on fd 3: {fds}"
    );
    let argv = dirs.recorded("argv");
    let config = dirs.work.path().join("mcp.json");
    let want = format!(
        "-p\n--no-session-persistence\n--model\nsonnet\n--output-format\nstream-json\n\
         --verbose\n--tools\n\n--setting-sources\n\n--disable-slash-commands\n\
         --strict-mcp-config\n--permission-prompts\nnone\n--system-prompt\nBe brief.\n\
         --mcp-config\n{}\n--allowedTools\nmcp__moor\n",
        config.display()
    );
    assert_eq!(argv, want);
    assert_eq!(dirs.recorded("stdin"), "## New\nhello");
    let mut env: Vec<String> = dirs.recorded("env").lines().map(str::to_owned).collect();
    env.sort();
    let want = [
        "CLAUDE_CODE_DISABLE_CLAUDE_MDS=1".to_owned(),
        "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1".to_owned(),
        "CLAUDE_CODE_MAX_RETRIES=2".to_owned(),
        "CLAUDE_CODE_OAUTH_TOKEN=moor-proxy".to_owned(),
        format!("CLAUDE_CONFIG_DIR={}", dirs.config.path().display()),
        "HOME=/home/op".to_owned(),
        format!("HTTPS_PROXY=http://lease:{token}@127.0.0.1:4433"),
        "LANG=C.UTF-8".to_owned(),
        "NODE_EXTRA_CA_CERTS=/srv/moor/proxy-ca.pem".to_owned(),
        "PATH=/usr/local/bin:/usr/bin:/bin".to_owned(),
        "USER=op".to_owned(),
    ];
    assert_eq!(env, want);
    let cwd = dirs.recorded("cwd");
    assert_eq!(cwd.trim_end(), "/");
    for file in ["argv", "stdin", "cwd"] {
        assert!(!dirs.recorded(file).contains(token), "{file}");
    }
}

#[test]
fn the_turn_names_the_tool_server_in_a_private_mcp_json_with_no_variables() {
    let dirs = Dirs::new();
    dirs.turn("sk-ant-oat01-good-51d0", LIMITS);
    let config = dirs.work.path().join("mcp.json");
    let written: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&config).unwrap()).unwrap();
    let want = serde_json::json!({"mcpServers": {"moor": {
        "type": "stdio",
        "command": "/opt/moor/moor-slack",
        "args": ["mcp", "--thread", "C1/1.0"],
        "env": {},
    }}});
    assert_eq!(
        written, want,
        "the tool server gets no variables of its own"
    );
    let mode =
        std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(&config).unwrap().permissions());
    assert_eq!(mode & 0o777, 0o600);
}

#[test]
fn an_init_that_lists_a_tool_kills_the_turn() {
    let dirs = Dirs::new();
    let started = now();
    let (ended, pid) = dirs.turn("sk-ant-oat01-tool-1", LIMITS);
    let why = "the agent loaded the tool Bash".to_owned();
    assert_eq!(ended, Ended::Unsafe(why));
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "the turn ran on"
    );
    assert_gone(pid);
}

#[test]
fn an_init_that_lists_only_the_fronts_tool_server_and_its_tools_runs() {
    let dirs = Dirs::new();
    let (ended, _) = dirs.turn("sk-ant-oat01-listed-1", LIMITS);
    assert!(
        matches!(ended, Ended::Answered(Answer::Reply { .. })),
        "{ended:?}"
    );
}

#[test]
fn an_init_that_lists_another_tool_server_or_a_tool_the_front_did_not_give_kills_the_turn() {
    let dirs = Dirs::new();
    let (ended, _) = dirs.turn("sk-ant-oat01-server-1", LIMITS);
    let why = "the agent loaded the tool server github".to_owned();
    assert_eq!(ended, Ended::Unsafe(why));
    let dirs = Dirs::new();
    let (ended, _) = dirs.turn("sk-ant-oat01-extra-1", LIMITS);
    let why = "the agent loaded the tool mcp__moor__steer_box".to_owned();
    assert_eq!(ended, Ended::Unsafe(why));
}

#[test]
fn builtin_plugins_run_and_any_other_plugin_kills_the_turn_before_it_does_anything() {
    let dirs = Dirs::new();
    let (ended, _) = dirs.turn("sk-ant-oat01-plugin-builtin-1", LIMITS);
    assert!(
        matches!(ended, Ended::Answered(Answer::Reply { .. })),
        "{ended:?}"
    );
    let dirs = Dirs::new();
    let started = now();
    let (ended, pid) = dirs.turn("sk-ant-oat01-plugin-local-1", LIMITS);
    assert_eq!(
        ended,
        Ended::Unsafe("the agent loaded the plugin locplug".to_owned())
    );
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "the turn ran on"
    );
    assert_gone(pid);
    let dirs = Dirs::new();
    let (ended, _) = dirs.turn("sk-ant-oat01-plugin-spoof-1", LIMITS);
    let why = "the agent loaded the plugin spoof".to_owned();
    assert_eq!(
        ended,
        Ended::Unsafe(why),
        "a builtin source on a real path is not built in"
    );
    let dirs = Dirs::new();
    let (ended, _) = dirs.turn("sk-ant-oat01-plugin-bare-1", LIMITS);
    let why = "the agent loaded the plugin nosource".to_owned();
    assert_eq!(
        ended,
        Ended::Unsafe(why),
        "a plugin with no source is not built in"
    );
}

#[test]
fn the_cap_kills_a_turn_with_its_group() {
    let dirs = Dirs::new();
    let started = now();
    let (ended, pid) = dirs.turn("sk-ant-oat01-hang-1", SHORT);
    assert_eq!(ended, Ended::Killed);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "the turn ran on"
    );
    assert_gone(pid);
}

#[test]
fn a_401_is_a_refused_credential_and_no_login_is_an_unread_pipe() {
    let dirs = Dirs::new();
    let (ended, _) = dirs.turn("sk-ant-oat01-made-up", LIMITS);
    assert_eq!(ended, Ended::Answered(Answer::Refused));
    let dirs = Dirs::new();
    let (ended, _) = dirs.turn("", LIMITS);
    let why = "Claude Code 2.1.304 did not take the Proxy's placeholder login".to_owned();
    assert_eq!(ended, Ended::Answered(Answer::Unavailable(why)));
}

#[test]
fn a_reply_with_no_load_report_before_it_is_unsafe() {
    let dirs = Dirs::new();
    let (ended, pid) = dirs.turn("sk-ant-oat01-bare-1", LIMITS);
    let why = "a reply came before the load report".to_owned();
    assert_eq!(ended, Ended::Unsafe(why));
    assert_gone(pid);
}

/// Waits up to 2 s for `pid` to be gone or a zombie.
fn assert_gone(pid: u32) {
    let stat = PathBuf::from(format!("/proc/{pid}/stat"));
    for _ in 0..100 {
        let alive = std::fs::read_to_string(&stat).is_ok_and(|s| !s.contains(") Z "));
        if !alive {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("the turn's process outlived it");
}

/// The current instant, to time one turn.
#[allow(clippy::disallowed_methods, reason = "the test times a real process")]
fn now() -> std::time::Instant {
    std::time::Instant::now()
}
