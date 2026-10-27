use std::collections::BTreeMap;

use super::reconcile;
use crate::boxes::{Activity, Phase, Shown};
use crate::error::Error;
use crate::herdr::{Created, Herdr, Space};

#[derive(Default)]
struct Fake {
    spaces: Vec<Space>,
    calls: Vec<String>,
}

impl Fake {
    fn space(&mut self, id: &str) -> &mut Space {
        self.spaces.iter_mut().find(|s| s.id == id).unwrap()
    }

    fn sync(&mut self, boxes: &[Shown]) -> Vec<String> {
        self.calls.clear();
        reconcile(self, boxes, &|name| format!("attach {name}")).unwrap();
        std::mem::take(&mut self.calls)
    }
}

impl Herdr for Fake {
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

    fn tag(&mut self, workspace: &str, tokens: &[(&str, &str)]) -> Result<(), Error> {
        let pairs: Vec<String> = tokens.iter().map(|(k, v)| format!("{k}={v}")).collect();
        self.calls
            .push(format!("tag {workspace} {}", pairs.join(",")));
        let space = self.space(workspace);
        for (k, v) in tokens {
            if v.is_empty() {
                space.tokens.remove(*k);
            } else {
                space.tokens.insert((*k).to_owned(), (*v).to_owned());
            }
        }
        Ok(())
    }

    fn first_pane(&mut self, workspace: &str) -> Result<Option<String>, Error> {
        Ok(Some(format!("{workspace}:p1")))
    }

    fn report_agent(&mut self, pane: &str, state: &str) -> Result<(), Error> {
        self.calls.push(format!("agent {pane} {state}"));
        let id = pane.split(':').next().unwrap().to_owned();
        self.space(&id).agent_status = state.to_owned();
        Ok(())
    }
}

fn fix(phase: Phase) -> Shown {
    Shown {
        name: "fix".into(),
        repo: Some("oursky/moor".into()),
        branch: None,
        phase,
    }
}

const WORKING: Phase = Phase::Live(Activity::Working);

#[test]
fn a_new_box_gets_a_tagged_workspace_whose_pane_attaches() {
    let mut herdr = Fake::default();
    assert_eq!(
        herdr.sync(&[fix(WORKING)]),
        [
            "create ◈ moor/fix",
            "run w1:p1 attach fix",
            "tag w1 env=BOX,state=",
            "agent w1:p1 working"
        ]
    );
}

#[test]
fn syncing_again_changes_nothing() {
    let mut herdr = Fake::default();
    herdr.sync(&[fix(WORKING)]);
    assert!(herdr.sync(&[fix(WORKING)]).is_empty());
}

#[test]
fn a_box_that_closed_keeps_its_workspace_marked_closed() {
    let mut herdr = Fake::default();
    herdr.sync(&[fix(WORKING)]);
    assert_eq!(
        herdr.sync(&[]),
        ["tag w1 env=BOX,state=✕", "agent w1:p1 unknown"]
    );
    assert!(herdr.sync(&[]).is_empty());
    assert_eq!(herdr.spaces.len(), 1);
}

#[test]
fn a_state_change_retags_and_moves_the_agent_dot() {
    let mut herdr = Fake::default();
    herdr.sync(&[fix(WORKING)]);
    assert_eq!(
        herdr.sync(&[fix(Phase::Asleep)]),
        ["tag w1 env=BOX,state=◐", "agent w1:p1 unknown"]
    );
    assert_eq!(
        herdr.sync(&[fix(Phase::WaitingForMachine)]),
        ["tag w1 env=BOX,state=◌"]
    );
}

#[test]
fn tokens_herdr_forgot_on_restart_are_reported_again() {
    let mut herdr = Fake::default();
    herdr.sync(&[fix(WORKING)]);
    herdr.space("w1").tokens.clear();
    assert_eq!(herdr.sync(&[fix(WORKING)]), ["tag w1 env=BOX,state="]);
}

#[test]
fn workspaces_that_are_not_boxes_are_left_alone() {
    let mut herdr = Fake::default();
    herdr.create("moor").unwrap();
    assert!(herdr.sync(&[]).is_empty());
}
