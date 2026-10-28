use std::collections::{BTreeMap, VecDeque};
use std::time::Duration;

use moor_core::request::SecretValue;
use moor_core::user::SecretName;
use moor_core::view::user::StartNeedsValue;

use super::{accepted, refusal, run, start_request, Bench, Controller, Prompt, Start};
use crate::boxes::{Activity, Phase, Shown};
use crate::connect::Vault;
use crate::error::Error;
use crate::herdr::{Created, Herdr, Space};

#[derive(Default)]
struct Screen {
    answers: VecDeque<String>,
    said: Vec<String>,
    hidden: VecDeque<String>,
    asked_hidden: Vec<String>,
}

impl Screen {
    fn typing(answers: &[&str]) -> Self {
        Self {
            answers: answers.iter().map(|a| (*a).to_owned()).collect(),
            said: Vec::new(),
            hidden: VecDeque::new(),
            asked_hidden: Vec::new(),
        }
    }

    fn with_tokens(mut self, tokens: &[&str]) -> Self {
        self.hidden = tokens.iter().map(|t| (*t).to_owned()).collect();
        self
    }
}

impl Prompt for Screen {
    fn ask(&mut self, _question: &str) -> Result<Option<String>, Error> {
        Ok(self.answers.pop_front())
    }

    fn ask_hidden(&mut self, question: &str) -> Result<Option<String>, Error> {
        self.asked_hidden.push(question.to_owned());
        Ok(self.hidden.pop_front())
    }

    fn say(&mut self, line: &str) -> Result<(), Error> {
        self.said.push(line.to_owned());
        Ok(())
    }
}

struct Ctl {
    open: Vec<Shown>,
    started: std::cell::RefCell<Vec<Start>>,
    /// Secrets the person must store before a start is accepted.
    lacks: Vec<SecretName>,
    /// Every call that stores something, in order, without values.
    vault: std::cell::RefCell<Vec<String>>,
    /// What each stored secret was set to.
    values: std::cell::RefCell<Vec<(String, String)>>,
    /// Start requests, accepted or not.
    attempts: std::cell::Cell<usize>,
}

impl Ctl {
    fn lacking(names: &[&str]) -> Self {
        Self {
            lacks: names
                .iter()
                .map(|n| SecretName::parse(n).unwrap())
                .collect(),
            ..ctl()
        }
    }

    fn missing(&self) -> Vec<SecretName> {
        let done = self.values.borrow();
        self.lacks
            .iter()
            .filter(|n| !done.iter().any(|(d, _)| d == n.as_str()))
            .cloned()
            .collect()
    }

    fn needs(&self) -> StartNeedsValue {
        StartNeedsValue {
            identity: moor_core::user::Identity::local(1),
            user: self
                .vault
                .borrow()
                .iter()
                .any(|c| c == "enroll")
                .then(|| serde_json::from_value(serde_json::json!(1)).unwrap()),
            needs: self.lacks.clone(),
            missing: self.missing(),
        }
    }
}

impl Vault for Ctl {
    fn needs(&self) -> Result<StartNeedsValue, Error> {
        Ok(self.needs())
    }

    fn names(&self) -> Result<Vec<SecretName>, Error> {
        Ok(Vec::new())
    }

    fn enroll(&self) -> Result<(), Error> {
        self.vault.borrow_mut().push("enroll".to_owned());
        Ok(())
    }

    fn store(&self, name: &SecretName, value: SecretValue) -> Result<(), Error> {
        self.vault
            .borrow_mut()
            .push(format!("set {name}", name = name.as_str()));
        self.values.borrow_mut().push((
            name.as_str().to_owned(),
            String::from_utf8(value.secret().expose().to_vec()).unwrap(),
        ));
        Ok(())
    }
}

impl Controller for Ctl {
    fn open(&self) -> Result<Vec<Shown>, Error> {
        let mut now = self.open.clone();
        now.extend(self.started.borrow().iter().map(|s| live(&s.name, &s.repo)));
        Ok(now)
    }

    fn start(&self, start: &Start) -> Result<(), Error> {
        self.attempts.set(self.attempts.get() + 1);
        if !self.missing().is_empty() {
            return Err(Error::LoginRequired(Box::new(self.needs())));
        }
        self.started.borrow_mut().push(start.clone());
        Ok(())
    }
}

fn live(name: &str, repo: &str) -> Shown {
    Shown {
        name: name.to_owned(),
        repo: Some(repo.to_owned()),
        branch: None,
        phase: Phase::Live(Activity::Idle),
    }
}

fn ctl() -> Ctl {
    Ctl {
        open: vec![
            live("fix", "acme/app"),
            live("docs", "acme/site"),
            live("more", "acme/app"),
        ],
        started: std::cell::RefCell::default(),
        lacks: Vec::new(),
        vault: std::cell::RefCell::default(),
        values: std::cell::RefCell::default(),
        attempts: std::cell::Cell::new(0),
    }
}

#[derive(Default)]
struct Herd {
    spaces: Vec<Space>,
    calls: Vec<String>,
}

impl Herdr for Herd {
    fn spaces(&mut self) -> Result<Vec<Space>, Error> {
        Ok(self.spaces.clone())
    }

    fn create(&mut self, label: &str) -> Result<Created, Error> {
        let id = format!("w{}", self.spaces.len() + 1);
        self.spaces.push(Space {
            id: id.clone(),
            label: label.to_owned(),
            tokens: BTreeMap::new(),
            agent_status: "unknown".to_owned(),
        });
        self.calls.push(format!("create {label}"));
        Ok(Created {
            pane: format!("{id}:p1"),
            workspace: id,
        })
    }

    fn run(&mut self, pane: &str, command: &str) -> Result<(), Error> {
        self.calls.push(format!("run {pane} {command}"));
        Ok(())
    }

    fn tag(&mut self, _: &str, _: &[(&str, &str)]) -> Result<(), Error> {
        Ok(())
    }

    fn first_pane(&mut self, workspace: &str) -> Result<Option<String>, Error> {
        Ok(Some(format!("{workspace}:p1")))
    }

    fn report_agent(&mut self, _: &str, _: &str) -> Result<(), Error> {
        Ok(())
    }
}

impl Bench for Herd {
    fn tab(&mut self, workspace: &str) -> Result<(), Error> {
        self.calls.push(format!("tab {workspace}"));
        Ok(())
    }

    fn worktree(&mut self, workspace: &str, name: &str) -> Result<(), Error> {
        self.calls.push(format!("worktree {workspace} {name}"));
        Ok(())
    }

    fn focus(&mut self, workspace: &str) -> Result<(), Error> {
        self.calls.push(format!("focus {workspace}"));
        Ok(())
    }
}

fn pick(answers: &[&str]) -> (Screen, Ctl, Herd) {
    let (screen, ctl) = (Screen::typing(answers), ctl());
    pick_with(screen, ctl)
}

fn pick_with(mut screen: Screen, ctl: Ctl) -> (Screen, Ctl, Herd) {
    let mut herd = Herd::default();
    run(&mut screen, &ctl, &mut herd, "w1", &|n| {
        format!("attach {n}")
    })
    .unwrap();
    (screen, ctl, herd)
}

#[test]
fn local_with_no_name_opens_the_plain_tab() {
    let (_, _, herd) = pick(&["local", ""]);
    assert_eq!(herd.calls, ["tab w1"]);
}

#[test]
fn local_with_a_name_makes_a_worktree() {
    let (_, _, herd) = pick(&["1", "spike"]);
    assert_eq!(herd.calls, ["worktree w1 spike"]);
}

#[test]
fn a_worktree_name_an_open_box_has_is_refused_with_the_reason() {
    let (screen, _, herd) = pick(&["local", "fix", "spike"]);
    assert!(
        screen
            .said
            .iter()
            .any(|l| l.contains("fix") && l.contains("open Box")),
        "{:?}",
        screen.said
    );
    assert_eq!(herd.calls, ["worktree w1 spike"]);
}

#[test]
fn a_box_starts_on_a_known_repo_then_appears_and_takes_focus() {
    let (screen, ctl, herd) = pick(&["box", "2", "tidy", "", "fix the login page"]);
    assert!(
        screen.said.iter().any(|l| l.contains("1) acme/app")),
        "{:?}",
        screen.said
    );
    assert!(screen.said.iter().any(|l| l.contains("2) acme/site")));
    let started: Vec<_> = ctl
        .started
        .borrow()
        .iter()
        .map(|s| (s.name.clone(), s.repo.clone()))
        .collect();
    assert_eq!(started, [("tidy".to_owned(), "acme/site".to_owned())]);
    assert_eq!(
        herd.calls.last().map(String::as_str),
        Some("focus w4"),
        "{:?}",
        herd.calls
    );
    assert!(herd.calls.contains(&"run w4:p1 attach tidy".to_owned()));
}

#[test]
fn a_repo_can_be_typed_when_no_listed_one_fits() {
    let (_, ctl, _) = pick(&["2", "acme/new", "tidy", "", "fix it"]);
    assert_eq!(
        ctl.started.borrow().first().map(|s| s.repo.as_str()),
        Some("acme/new")
    );
}

#[test]
fn a_bad_choice_asks_again() {
    let (screen, _, herd) = pick(&["maybe", "local", ""]);
    assert!(
        screen.said.iter().any(|l| l.contains("local or box")),
        "{:?}",
        screen.said
    );
    assert_eq!(herd.calls, ["tab w1"]);
}

#[test]
fn an_empty_base_and_prompt_leave_both_to_the_controller() {
    let (_, ctl, _) = pick(&["box", "2", "tidy", " ", ""]);
    let started = ctl.started.borrow();
    let start = started.first().unwrap();
    assert_eq!(
        (start.base.as_deref(), start.prompt.as_deref()),
        (None, None)
    );
    assert_eq!(started.len(), 1);
}

#[test]
fn a_given_base_reaches_the_controller() {
    let (_, ctl, _) = pick(&["box", "2", "tidy", " release/2.1 ", ""]);
    assert_eq!(
        ctl.started.borrow().first().and_then(|s| s.base.clone()),
        Some("release/2.1".to_owned())
    );
}

#[test]
fn a_given_prompt_reaches_the_controller() {
    let (_, ctl, _) = pick(&["box", "2", "tidy", "", " fix the login page "]);
    assert_eq!(
        ctl.started.borrow().first().and_then(|s| s.prompt.clone()),
        Some("fix the login page".to_owned())
    );
}

fn tidy() -> Start {
    Start {
        name: "tidy".to_owned(),
        repo: "acme/site".to_owned(),
        ..Start::default()
    }
}

#[test]
fn the_start_request_without_a_base_or_prompt_sends_neither() {
    let wire = serde_json::to_value(start_request(&tidy()).unwrap()).unwrap();
    assert_eq!(
        wire,
        serde_json::json!({"cmd": "start", "name": "tidy", "repo": "acme/site",
            "no_agent": false, "resume": false})
    );
}

#[test]
fn the_start_request_carries_the_base_and_prompt() {
    let start = Start {
        base: Some("release/2.1".to_owned()),
        prompt: Some("fix it".to_owned()),
        ..tidy()
    };
    let wire = serde_json::to_value(start_request(&start).unwrap()).unwrap();
    assert_eq!(
        wire,
        serde_json::json!({"cmd": "start", "name": "tidy", "repo": "acme/site",
            "base": "release/2.1", "prompt": "fix it", "no_agent": false, "resume": false})
    );
}

#[test]
fn a_start_is_accepted_once_its_box_is_listed_while_the_start_runs_on() {
    let (_running, outcome) = std::sync::mpsc::channel();
    let mut polls = 0;
    let listed = || {
        polls += 1;
        polls == 3
    };
    assert!(accepted(&outcome, Duration::ZERO, listed).is_ok());
    assert_eq!(polls, 3);
}

#[test]
fn a_refused_start_fails_with_the_refusal_before_its_box_is_listed() {
    let (done, outcome) = std::sync::mpsc::channel();
    done.send(Err(Error::Refused("name taken".into()))).unwrap();
    let got = accepted(&outcome, Duration::ZERO, || false);
    assert_eq!(
        got.unwrap_err().to_string(),
        Error::Refused("name taken".into()).to_string()
    );
}

#[test]
fn a_start_that_ends_before_a_poll_sees_its_box_is_accepted() {
    let (done, outcome) = std::sync::mpsc::channel();
    done.send(Ok(())).unwrap();
    assert!(accepted(&outcome, Duration::ZERO, || false).is_ok());
}

const BOX: [&str; 5] = ["box", "1", "tidy", "", "fix it"];

#[test]
fn a_start_refused_for_missing_secrets_asks_for_each_stores_it_and_starts_again() {
    let screen = Screen::typing(&BOX).with_tokens(&["gho_one", "sk-ant-two"]);
    let (screen, ctl, herd) = pick_with(screen, Ctl::lacking(&["github", "claude"]));
    assert_eq!(
        *ctl.vault.borrow(),
        ["enroll", "set github", "set claude"],
        "in the order the controller lists them"
    );
    assert_eq!(
        *ctl.values.borrow(),
        [
            ("github".to_owned(), "gho_one".to_owned()),
            ("claude".to_owned(), "sk-ant-two".to_owned())
        ]
    );
    assert_eq!(ctl.attempts.get(), 2, "one refusal, one retry");
    assert_eq!(ctl.started.borrow().len(), 1);
    assert!(screen.said.iter().any(|l| l.contains("gh auth token")));
    assert!(screen.said.iter().any(|l| l.contains("claude setup-token")));
    assert_eq!(screen.asked_hidden.len(), 2);
    assert!(herd.calls.iter().any(|c| c.starts_with("focus")));
}

#[test]
fn a_value_is_never_among_the_lines_the_picker_shows() {
    let screen = Screen::typing(&BOX).with_tokens(&["gho_topsecret"]);
    let (screen, _, _) = pick_with(screen, Ctl::lacking(&["github"]));
    let shown = format!("{:?} {:?}", screen.said, screen.asked_hidden);
    assert!(!shown.contains("gho_topsecret"), "{shown}");
}

#[test]
fn a_start_with_nothing_missing_asks_for_no_token() {
    let (screen, ctl, _) = pick(&BOX);
    assert!(screen.asked_hidden.is_empty());
    assert!(ctl.vault.borrow().is_empty());
    assert_eq!(ctl.attempts.get(), 1);
}

#[test]
fn giving_up_on_the_tokens_starts_nothing() {
    let screen = Screen::typing(&BOX);
    let (_, ctl, herd) = pick_with(screen, Ctl::lacking(&["github"]));
    assert!(ctl.started.borrow().is_empty());
    assert_eq!(ctl.attempts.get(), 1);
    assert!(herd.calls.is_empty());
}

fn refused(
    detail: Option<&serde_json::Value>,
    value: &serde_json::Value,
) -> moor_core::response::Response {
    serde_json::from_value(serde_json::json!({
        "schema": "moor/1", "cmd": "start", "class": "refused", "value": value, "warnings": [],
        "error": {"kind": "refused", "message": "log in first", "hints": [], "context": [],
                  "detail": detail},
    }))
    .unwrap()
}

#[test]
fn a_login_required_refusal_keeps_what_is_missing() {
    let value = serde_json::json!({"identity": "local:1", "user": null,
        "needs": ["github", "claude"], "missing": ["github", "claude"]});
    let detail = serde_json::json!({"kind": "login_required"});
    let err = refusal(refused(Some(&detail), &value)).unwrap_err();
    assert!(
        matches!(&err, Error::LoginRequired(needs) if needs.missing.len() == 2),
        "{err}"
    );
    let other = refusal(refused(None, &value)).unwrap_err();
    assert!(matches!(other, Error::Refused(m) if m == "log in first"));
}
