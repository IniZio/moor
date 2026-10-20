//! Tests for the composition root.

use super::*;
use crate::testing::clock::FakeClock;
use crate::testing::proc::{Reply, ScriptedProc};
use moor_core::activity::PaneStatus;
use moor_core::name::BoxName;
use moor_core::seam::clock::Ts;
use moor_core::seam::forge::{Forge, ForgeRepo};
use moor_core::seam::proc::Tool;
use moor_core::seam::surface::{PaneRef, ResumeReg};

fn window(value: Option<&str>) -> Duration {
    let pairs: Vec<(&str, &str)> = value.map(|v| (PROMPT_WINDOW_ENV, v)).into_iter().collect();
    prompt_window(&Env::from_pairs(pairs))
}

#[test]
fn the_herdr_adapter_numbers_its_reports_from_the_clock() {
    let proc = Arc::new(ScriptedProc::new());
    proc.on(Tool::Herdr, &["pane"], Reply::Ok(String::new()));
    let clock = Arc::new(FakeClock::new(Ts::from_millis(7_000)));
    let h = herdr(&(proc.clone() as Arc<dyn Proc>), "s", Vec::new(), clock);
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
fn the_github_forge_defaults_to_gh_auth_token_and_passes_set_variables() {
    let proc = Arc::new(ScriptedProc::new());
    proc.on(Tool::Sh, &["-c"], Reply::Ok("tok\n".to_owned()));
    let mut config = UserConfig::default();
    config.github.pass_env = vec!["SET_ME".to_owned(), "UNSET_ME".to_owned()];
    let env = Env::from_pairs([("SET_ME", "v")]);
    let g = github(&(proc.clone() as Arc<dyn Proc>), &config, &env);
    g.repo_token(&ForgeRepo::new("o", "n")).unwrap();
    let calls = proc.calls();
    let [call] = calls.as_slice() else {
        return assert_eq!(calls.len(), 1);
    };
    assert_eq!(
        call.args.get(3..),
        Some(&["gh", "auth", "token"].map(str::to_owned)[..])
    );
    assert_eq!(call.env, [("SET_ME".to_owned(), "v".to_owned())]);
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
