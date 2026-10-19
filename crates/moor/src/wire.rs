//! Composition root: the only module that names concrete adapters.

use std::collections::BTreeMap;
use std::io::Write;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use moor_core::BoxId;
use signal_hook::consts::{SIGINT, SIGTERM};

use moor_core::config::{DriverConfig, UserConfig};
use moor_core::seam::clock::Clock;
use moor_core::seam::driver::Driver;
use moor_core::seam::proc::Proc;
use moor_core::seam::rng::Rng;
use moor_core::seam::surface::Surface;
use moor_core::ErrorKind;

use crate::adapters::box_agent::claude::Claude;
use crate::adapters::box_agent::gh_release::{GhRelease, UreqFetch};
use crate::adapters::box_agent::BoxAgent;
use crate::adapters::clock::SystemClock;
use crate::adapters::crabbox::Crabbox;
use crate::adapters::creds::Creds;
use crate::adapters::git::Git;
use crate::adapters::github::GitHub;
use crate::adapters::herdr::Herdr;
use crate::adapters::proc::{SystemProc, ToolPaths};
use crate::adapters::rng::SystemRng;
use crate::config_load::load_user;
use crate::cx::{Cx, CxParts};
use crate::env::Env;
use crate::error::CommandError;
use crate::proxy::adopt;
use crate::proxy::detached::DetachedSupervisor;
use crate::proxy::nono::capture::CredsFn;
use crate::proxy::{serve, Deps, Sink};

/// Host variables herdr needs to find its socket and terminal.
const HERDR_PASSTHROUGH: [&str; 5] = [
    "XDG_RUNTIME_DIR",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_STATE_HOME",
    "TERM",
];

/// The herdr adapter, numbering its reports from `clock`.
fn herdr(
    proc: &Arc<dyn Proc>,
    session: &str,
    extra_env: Vec<(String, String)>,
    clock: Arc<dyn Clock>,
) -> Herdr {
    Herdr::new(Arc::clone(proc), session.to_owned(), extra_env).with_clock(clock)
}

/// Per-provider lookup deadlines for the providers that configure one.
fn lookup_deadlines(drivers: &BTreeMap<String, DriverConfig>) -> BTreeMap<String, Duration> {
    drivers
        .iter()
        .filter_map(|(provider, d)| {
            Some((
                provider.clone(),
                Duration::from_secs(u64::from(d.lookup_timeout_secs?.get())),
            ))
        })
        .collect()
}

/// Wire real adapters from the environment into a context.
pub(crate) fn build(env: &Env) -> Result<Cx, CommandError> {
    Ok(Cx::new(parts(env)?).with_repos(env.repos().clone()))
}

/// Settles the Proxy supervisors recorded in the state store; one line per failure.
pub(crate) fn reconcile_proxies(env: &Env) -> Vec<String> {
    let Some(state_dir) = env.state_dir() else {
        return Vec::new();
    };
    let clock = match SystemClock::from_env(env) {
        Ok(clock) => Arc::new(clock),
        Err(e) => return vec![e.to_string()],
    };
    let config = match load_user(env) {
        Ok(config) => config,
        Err(e) => return vec![e.error.to_string()],
    };
    let proc: Arc<dyn Proc> = Arc::new(SystemProc::new(env, &tool_paths(&config)));
    let host = DetachedSupervisor::new(
        Arc::clone(&proc),
        Arc::clone(&clock) as Arc<dyn Clock>,
        Arc::new(SystemRng),
        Some(state_dir.clone()),
    )
    .keeping_nonce();
    adopt::reconcile(&host, proc.as_ref(), clock, &state_dir)
}

/// Real adapters from the environment, not yet assembled into a context.
fn parts(env: &Env) -> Result<CxParts, CommandError> {
    let clock = SystemClock::from_env(env).map_err(|e| CommandError::new(ErrorKind::Usage, e))?;
    let config = load_user(env)?;
    let cwd = std::env::current_dir()
        .map_err(|e| CommandError::new(ErrorKind::Failed, format!("current directory: {e}")))?;
    let proc: Arc<dyn Proc> = Arc::new(SystemProc::new(env, &tool_paths(&config)));
    let extra_env = HERDR_PASSTHROUGH
        .iter()
        .filter_map(|k| Some(((*k).to_owned(), env.get(k)?.to_owned())))
        .collect();
    let clock = Arc::new(clock);
    let surface: Arc<dyn Surface> = Arc::new(herdr(
        &proc,
        config.herdr_session(),
        extra_env,
        Arc::clone(&clock) as Arc<dyn Clock>,
    ));
    let rng: Arc<dyn Rng> = Arc::new(SystemRng);
    let driver: Arc<dyn Driver> = Arc::new(
        Crabbox::new(
            Arc::clone(&proc),
            Arc::clone(&clock) as Arc<dyn Clock>,
            Arc::clone(&rng),
        )
        .with_lookup_deadlines(lookup_deadlines(&config.drivers)),
    );
    let engine = BoxAgent::new(
        Arc::clone(&driver),
        Arc::clone(&surface),
        Arc::new(Claude),
        Arc::clone(&clock) as Arc<dyn Clock>,
        Arc::new(GhRelease::new(
            Arc::new(UreqFetch),
            env.xdg("XDG_CACHE_HOME", ".cache", "moor"),
        )),
    );
    let state_dir = env.state_dir();
    let proxy_host = Arc::new(DetachedSupervisor::new(
        Arc::clone(&proc),
        Arc::clone(&clock) as Arc<dyn Clock>,
        Arc::clone(&rng),
        state_dir.clone(),
    ));
    Ok(CxParts {
        state_dir,
        clock,
        rng,
        driver,
        surface,
        engine: Arc::new(engine),
        proc: Arc::clone(&proc),
        forge: Arc::new(github(&proc, &config, env)),
        proxy_host,
        git: Git::new(Arc::clone(&proc)),
        config,
        creds: Creds::new(env.clone(), proc),
        cwd,
        prompt_window: prompt_window(env),
    })
}

/// Run the credential proxy for `task` until a stop signal; returns the exit code.
///
/// Setup failures are written to `stderr` as one `moor proxy: <error>` line.
pub fn serve_proxy(env: &Env, task: BoxId, stderr: impl Write + Send + 'static) -> u8 {
    let sink: Sink = Arc::new(Mutex::new(stderr));
    match run_proxy(env, task, &sink) {
        Ok(code) => code,
        Err(e) => {
            // The sink is the only channel left; a failed write has nowhere to go.
            let _ = writeln!(
                sink.lock().unwrap_or_else(PoisonError::into_inner),
                "moor proxy: {e}"
            );
            1
        }
    }
}

/// Builds the proxy's dependencies and serves; `Err` is a setup failure.
fn run_proxy(env: &Env, task: BoxId, sink: &Sink) -> Result<u8, String> {
    let parts = parts(env).map_err(|e| e.error.to_string())?;
    let forge = Arc::clone(&parts.forge);
    let clock = Arc::clone(&parts.clock);
    let proc = Arc::clone(&parts.proc);
    let cx = Cx::new(parts);
    let store = cx.store().map_err(|e| e.error.to_string())?;
    let state_dir = cx.state_dir().map_err(|e| e.error.to_string())?;
    let stop = Arc::new(AtomicBool::new(false));
    for sig in [SIGTERM, SIGINT] {
        signal_hook::flag::register(sig, Arc::clone(&stop)).map_err(|e| e.to_string())?;
    }
    let sources = cx.config().credentials.clone();
    let reader = Creds::new(env.clone(), Arc::clone(&proc));
    let creds: CredsFn = Arc::new(move |name: &str| {
        sources.get(name).map_or_else(
            || Err(format!("credential {name} is not configured")),
            |src| reader.secret(src),
        )
    });
    let deps = Deps {
        store,
        driver: cx.driver(),
        forge,
        clock,
        rng: cx.rng(),
        creds,
        upstream_ca: None,
        upstream_dial: None,
        guard_exempt: Vec::new(),
        state_dir,
        boot_id: proc.boot_id(),
        stop,
        stderr: Arc::clone(sink),
    };
    Ok(serve(&deps, task))
}

/// The GitHub forge; `pass_env` names unset in `env` are skipped.
fn github(proc: &Arc<dyn Proc>, config: &UserConfig, env: &Env) -> GitHub {
    let gh = &config.github;
    let pairs = gh
        .pass_env
        .iter()
        .filter_map(|k| Some((k.clone(), env.get(k)?.to_owned())))
        .collect();
    GitHub::new(Arc::clone(proc), gh.token_cmd(), pairs)
}

/// The environment variable that sets the first-run prompt watch window, in milliseconds.
const PROMPT_WINDOW_ENV: &str = "MOOR_AGENT_PROMPT_WINDOW_MS";

/// Window used when the variable is unset or not a number.
const DEFAULT_PROMPT_WINDOW_MS: u64 = 8_000;

/// How long to watch a fresh agent pane for first-run prompts.
fn prompt_window(env: &Env) -> Duration {
    let ms = env
        .get(PROMPT_WINDOW_ENV)
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_PROMPT_WINDOW_MS);
    Duration::from_millis(ms)
}

/// Tool overrides from config.
fn tool_paths(config: &UserConfig) -> ToolPaths {
    let t = &config.tools;
    ToolPaths {
        crabbox: t.crabbox.clone(),
        herdr: t.herdr.clone(),
        claude: t.claude.clone(),
        gh: t.gh.clone(),
        ssh: t.ssh.clone(),
        git: t.git.clone(),
    }
}

#[cfg(test)]
mod tests {
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
    fn serving_an_unknown_task_fails_with_one_stderr_line() {
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
}
