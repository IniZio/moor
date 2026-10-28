use std::cell::RefCell;
use std::collections::VecDeque;

use moor_core::request::SecretValue;
use moor_core::user::{Identity, SecretName};
use moor_core::view::user::StartNeedsValue;

use super::{guide, provide, run, Vault};
use crate::error::Error;
use crate::picker::Prompt;

#[derive(Default)]
struct Typist {
    tokens: VecDeque<String>,
    said: Vec<String>,
    asked: Vec<String>,
}

impl Typist {
    fn typing(tokens: &[&str]) -> Self {
        Self {
            tokens: tokens.iter().map(|t| (*t).to_owned()).collect(),
            ..Self::default()
        }
    }
}

impl Prompt for Typist {
    fn ask(&mut self, _: &str) -> Result<Option<String>, Error> {
        Ok(None)
    }

    fn ask_hidden(&mut self, question: &str) -> Result<Option<String>, Error> {
        self.asked.push(question.to_owned());
        Ok(self.tokens.pop_front())
    }

    fn say(&mut self, line: &str) -> Result<(), Error> {
        self.said.push(line.to_owned());
        Ok(())
    }
}

struct Store {
    needs: Vec<SecretName>,
    have: Vec<SecretName>,
    enrolled: bool,
    calls: RefCell<Vec<String>>,
}

fn names(list: &[&str]) -> Vec<SecretName> {
    list.iter().map(|n| SecretName::parse(n).unwrap()).collect()
}

impl Store {
    fn new(needs: &[&str], have: &[&str], enrolled: bool) -> Self {
        Self {
            needs: names(needs),
            have: names(have),
            enrolled,
            calls: RefCell::default(),
        }
    }
}

impl Vault for Store {
    fn needs(&self) -> Result<StartNeedsValue, Error> {
        Ok(StartNeedsValue {
            identity: Identity::local(1),
            user: self
                .enrolled
                .then(|| serde_json::from_value(serde_json::json!(1)).unwrap()),
            needs: self.needs.clone(),
            missing: self
                .needs
                .iter()
                .filter(|n| !self.have.contains(n))
                .cloned()
                .collect(),
        })
    }

    fn names(&self) -> Result<Vec<SecretName>, Error> {
        Ok(self.have.clone())
    }

    fn enroll(&self) -> Result<(), Error> {
        self.calls.borrow_mut().push("enroll".to_owned());
        Ok(())
    }

    fn store(&self, name: &SecretName, value: SecretValue) -> Result<(), Error> {
        let typed = String::from_utf8(value.secret().expose().to_vec()).unwrap();
        self.calls
            .borrow_mut()
            .push(format!("set {} {typed}", name.as_str()));
        Ok(())
    }
}

#[test]
fn the_guidance_is_the_one_slack_gives() {
    assert!(guide("github").1.contains("`gh auth token`"));
    assert!(guide("claude").1.contains("`claude setup-token`"));
    assert_eq!(
        guide("npm"),
        ("npm".to_owned(), "Paste the value of npm.".to_owned())
    );
}

#[test]
fn every_missing_secret_is_asked_for_in_order_after_enrolling() {
    let store = Store::new(&["github", "claude"], &[], false);
    let mut typist = Typist::typing(&["gho_a", "sk-ant-b"]);
    let needs = store.needs().unwrap();
    assert!(provide(&mut typist, &store, &needs).unwrap());
    assert_eq!(
        *store.calls.borrow(),
        ["enroll", "set github gho_a", "set claude sk-ant-b"]
    );
    assert!(typist
        .asked
        .first()
        .is_some_and(|q| q.starts_with("GitHub")));
    assert!(typist.asked.get(1).is_some_and(|q| q.starts_with("Claude")));
}

#[test]
fn a_secret_the_person_already_has_is_not_asked_for() {
    let store = Store::new(&["github", "claude"], &["claude"], true);
    let mut typist = Typist::typing(&["gho_a"]);
    let needs = store.needs().unwrap();
    assert!(provide(&mut typist, &store, &needs).unwrap());
    assert_eq!(*store.calls.borrow(), ["enroll", "set github gho_a"]);
}

#[test]
fn an_empty_answer_is_asked_again_and_the_end_of_input_gives_up() {
    let store = Store::new(&["github"], &[], true);
    let needs = store.needs().unwrap();
    let mut typist = Typist::typing(&["", "gho_a"]);
    assert!(provide(&mut typist, &store, &needs).unwrap());
    assert_eq!(typist.asked.len(), 2);
    let mut typist = Typist::typing(&[]);
    assert!(!provide(&mut typist, &store, &needs).unwrap());
}

#[test]
fn connect_shows_which_are_set_and_replaces_only_what_is_typed() {
    let store = Store::new(&["github", "claude"], &["github"], true);
    let mut typist = Typist::typing(&["", "sk-ant-new"]);
    run(&mut typist, &store).unwrap();
    assert!(typist.said.iter().any(|l| l.starts_with("GitHub: set.")));
    assert!(typist
        .said
        .iter()
        .any(|l| l.starts_with("Claude: not set.")));
    assert_eq!(*store.calls.borrow(), ["set claude sk-ant-new"]);
}

#[test]
fn connect_can_replace_a_secret_that_is_set() {
    let store = Store::new(&["github"], &["github"], true);
    let mut typist = Typist::typing(&["gho_rotated"]);
    run(&mut typist, &store).unwrap();
    assert_eq!(*store.calls.borrow(), ["set github gho_rotated"]);
}

#[test]
fn no_line_shown_holds_a_value() {
    let store = Store::new(&["github"], &[], false);
    let mut typist = Typist::typing(&["gho_topsecret"]);
    run(&mut typist, &store).unwrap();
    let shown = format!("{:?}{:?}", typist.said, typist.asked);
    assert!(!shown.contains("gho_topsecret"), "{shown}");
}
