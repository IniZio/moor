use std::collections::{BTreeMap, VecDeque};

use super::{run, start_request, Bench, Controller, Prompt};
use crate::boxes::{Activity, Phase, Shown};
use crate::error::Error;
use crate::herdr::{Created, Herdr, Space};

#[derive(Default)]
struct Screen {
    answers: VecDeque<String>,
    said: Vec<String>,
}

impl Screen {
    fn typing(answers: &[&str]) -> Self {
        Self {
            answers: answers.iter().map(|a| (*a).to_owned()).collect(),
            said: Vec::new(),
        }
    }
}

impl Prompt for Screen {
    fn ask(&mut self, _question: &str) -> Result<Option<String>, Error> {
        Ok(self.answers.pop_front())
    }

    fn say(&mut self, line: &str) -> Result<(), Error> {
        self.said.push(line.to_owned());
        Ok(())
    }
}

struct Ctl {
    open: Vec<Shown>,
    started: std::cell::RefCell<Vec<(String, String)>>,
    prompts: std::cell::RefCell<Vec<Option<String>>>,
}

impl Controller for Ctl {
    fn open(&self) -> Result<Vec<Shown>, Error> {
        let mut now = self.open.clone();
        now.extend(
            self.started
                .borrow()
                .iter()
                .map(|(name, repo)| live(name, repo)),
        );
        Ok(now)
    }

    fn start(&self, name: &str, repo: &str, prompt: Option<&str>) -> Result<(), Error> {
        self.prompts.borrow_mut().push(prompt.map(str::to_owned));
        self.started
            .borrow_mut()
            .push((name.to_owned(), repo.to_owned()));
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
        prompts: std::cell::RefCell::default(),
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
    let (mut screen, ctl, mut herd) = (Screen::typing(answers), ctl(), Herd::default());
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
    let (screen, ctl, herd) = pick(&["box", "2", "tidy", "fix the login page"]);
    assert!(
        screen.said.iter().any(|l| l.contains("1) acme/app")),
        "{:?}",
        screen.said
    );
    assert!(screen.said.iter().any(|l| l.contains("2) acme/site")));
    assert_eq!(
        *ctl.started.borrow(),
        [("tidy".to_owned(), "acme/site".to_owned())]
    );
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
    let (_, ctl, _) = pick(&["2", "acme/new", "tidy", "fix it"]);
    assert_eq!(
        ctl.started.borrow().first().map(|s| s.1.as_str()),
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
fn an_empty_prompt_starts_the_box_with_none() {
    let (_, ctl, _) = pick(&["box", "2", "tidy", ""]);
    assert_eq!(*ctl.prompts.borrow(), [None]);
    assert_eq!(ctl.started.borrow().len(), 1);
}

#[test]
fn a_given_prompt_reaches_the_controller() {
    let (_, ctl, _) = pick(&["box", "2", "tidy", " fix the login page "]);
    assert_eq!(
        *ctl.prompts.borrow(),
        [Some("fix the login page".to_owned())]
    );
}

#[test]
fn the_start_request_without_a_prompt_sends_none() {
    let wire = serde_json::to_value(start_request("tidy", "acme/site", None).unwrap()).unwrap();
    assert_eq!(
        wire,
        serde_json::json!({"cmd": "start", "name": "tidy", "repo": "acme/site",
            "no_agent": false, "resume": false})
    );
}

#[test]
fn the_start_request_carries_the_prompt() {
    let wire =
        serde_json::to_value(start_request("tidy", "acme/site", Some("fix it")).unwrap()).unwrap();
    assert_eq!(
        wire,
        serde_json::json!({"cmd": "start", "name": "tidy", "repo": "acme/site", "prompt": "fix it",
            "no_agent": false, "resume": false})
    );
}
