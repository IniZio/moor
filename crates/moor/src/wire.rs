//! Composition root: the only module that names concrete adapters.

use std::sync::Arc;
use std::time::Duration;

use moor_core::config::UserConfig;
use moor_core::seam::clock::Clock;
use moor_core::seam::driver::Driver;
use moor_core::seam::proc::Proc;
use moor_core::seam::surface::Surface;
use moor_core::ErrorKind;

use crate::adapters::box_agent::claude::Claude;
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

/// Wire real adapters from the environment into a context.
pub(crate) fn build(env: &Env) -> Result<Cx, CommandError> {
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
    let driver: Arc<dyn Driver> = Arc::new(Crabbox::new(Arc::clone(&proc)));
    let engine = BoxAgent::new(
        Arc::clone(&driver),
        Arc::clone(&surface),
        Arc::new(Claude),
        Arc::clone(&clock) as Arc<dyn Clock>,
    );
    Ok(Cx::new(CxParts {
        state_dir: env.state_dir(),
        clock,
        rng: Arc::new(SystemRng),
        driver,
        surface,
        engine: Arc::new(engine),
        proc: Arc::clone(&proc),
        forge: Arc::new(github(&proc, &config, env)),
        git: Git::new(Arc::clone(&proc)),
        config,
        creds: Creds::new(env.clone(), proc),
        cwd,
        prompt_window: prompt_window(env),
    }))
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
    use moor_core::name::TaskName;
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
            task: TaskName::parse("a").unwrap(),
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
