//! Where a reading is kept: written once, held with its gates in one transaction, and never read
//! through another person's workflow.

use std::collections::BTreeMap;
use std::path::PathBuf;

use moor_core::gate::Gates;
use moor_core::name::{BoxName, ForgeRepo};
use moor_core::reading::{
    Context, HandIn, ReaderRun, Reading, ReadingId, ReadingOf, ReadingState, RulesPin,
};
use moor_core::seam::clock::Ts;
use moor_core::user::{Identity, UserId};
use moor_core::workflow::{PromptHash, TimeoutMinutes, WorkflowDef, WorkflowId, WorkflowName};
use moor_schema::tables::Workflows;
use sea_query::{Alias, Table};
use serde_json::json;

use crate::store::db::testing;
use crate::store::readings::Stored;
use crate::store::tests::open;
use crate::store::{Store, StoreError};

const SOURCE: &str = "Take screenshots of each page. Post one comment on the pull request.";

struct Rig {
    store: Store,
    alice: UserId,
    bob: UserId,
    /// Alice's workflow.
    workflow: WorkflowId,
    path: PathBuf,
    _dir: tempfile::TempDir,
}

impl Rig {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let (store, _) = open(dir.path());
        let user = |who: &str| {
            store
                .enroll(&Identity::parse(who).unwrap(), None)
                .unwrap()
                .0
        };
        let (alice, bob) = (user("local:1"), user("local:2"));
        let def = WorkflowDef::new(
            WorkflowName::parse("evidence").unwrap(),
            None,
            vec![ForgeRepo::new("acme", "web")],
            SOURCE,
            (true, TimeoutMinutes::DEFAULT),
        )
        .unwrap();
        let workflow = store.put_workflow(alice, &def).unwrap();
        let path = dir.path().join("state").join("moor.db");
        Self {
            store,
            alice,
            bob,
            workflow,
            path,
            _dir: dir,
        }
    }

    fn gates(&self) -> Option<Gates> {
        self.store
            .workflow_by_id(self.workflow)
            .unwrap()
            .unwrap()
            .gates
    }

    /// Takes the column an update of the workflow names away, so that update fails as a full disk
    /// would, after the statement before it.
    fn break_workflow_updates(&self) {
        let mut alter = Table::alter();
        alter
            .table(Workflows::Table)
            .rename_column(Workflows::Reading, Alias::new("gone"));
        testing::open_conn(&self.path).schema(&alter).unwrap();
    }
}

fn effects() -> serde_json::Value {
    json!([
        {"kind": "gate", "label": "shots", "gate": {"name": "screenshots", "evidence": ["shot-*"]}},
        {"kind": "allow", "label": "comment", "rule": "github-app",
         "entry": "POST /repos/{repo}/issues/*/comments"}
    ])
}

fn held(exact: &str, label: &str) -> serde_json::Value {
    json!({"quote": {"exact": exact}, "mark": {"kind": "held", "effects": [label]}, "why": "moor holds it"})
}

/// A reading of Alice's workflow that is read: two held phrases, a gate and a `may` line.
fn read_reading(of: WorkflowId, by: UserId, id: &str) -> Reading {
    let reading = Reading {
        id: ReadingId::parse(id).unwrap(),
        of: ReadingOf::Automation(of),
        parent: None,
        source: SOURCE.into(),
        context: Context {
            source_hash: PromptHash::of(SOURCE),
            repos: vec![ForgeRepo::new("acme", "web")],
            rules: vec![RulesPin {
                repo: None,
                revision: 3,
            }],
            instructions: 1,
        },
        made_by: by,
        state: ReadingState::Reading {
            by: ReaderRun {
                box_name: BoxName::parse("acme/reader-1").unwrap(),
                harness: "claude".into(),
                model: None,
            },
            since: Ts::from_millis(1),
        },
    };
    let doc: HandIn = serde_json::from_value(json!({
        "phrases": [
            held("Take screenshots of each page", "shots"),
            held("Post one comment on the pull request", "comment"),
        ],
        "effects": effects(),
    }))
    .unwrap();
    let allow = BTreeMap::from([(
        "github-app".to_owned(),
        vec!["POST /repos/{repo}/issues/*/comments".to_owned()],
    )]);
    let read = reading.hand_in(&doc, &allow).unwrap();
    Reading {
        state: ReadingState::Read(read),
        ..reading
    }
}

const ID: &str = "01936a3e-7c2b-7d4e-8f00-0123456789ab";
const OTHER: &str = "01936a3e-7c2b-7d4e-8f00-0123456789ac";

#[test]
fn a_read_is_written_once() {
    let rig = Rig::new();
    let first = read_reading(rig.workflow, rig.alice, ID);
    assert_eq!(rig.store.insert_read(&first).unwrap(), Stored::Written);

    let mut changed = read_reading(rig.workflow, rig.alice, ID);
    changed.source = "Another prompt.".into();
    changed.state = ReadingState::Read(moor_core::reading::Read::from_gates(
        &Gates::parse(
            &[serde_json::from_value(json!({"name": "proof", "evidence": ["pr-*"]})).unwrap()],
            &[],
        )
        .unwrap()
        .doc(),
    ));
    assert_eq!(rig.store.insert_read(&changed).unwrap(), Stored::Differs);
    assert_eq!(
        rig.store
            .reading(rig.alice, &first.id)
            .unwrap()
            .unwrap()
            .state,
        first.state
    );
}

#[test]
fn writing_the_same_read_again_changes_nothing() {
    let rig = Rig::new();
    let first = read_reading(rig.workflow, rig.alice, ID);
    assert_eq!(rig.store.insert_read(&first).unwrap(), Stored::Written);
    assert_eq!(rig.store.insert_read(&first).unwrap(), Stored::Same);
    assert_eq!(rig.store.hold_read(&first).unwrap(), Stored::Same);
    assert_eq!(rig.store.hold_read(&first).unwrap(), Stored::Same);
    assert_eq!(
        rig.store.workflow_reading(rig.alice, rig.workflow).unwrap(),
        Some(first.id)
    );
}

#[test]
fn only_a_read_reading_is_kept_in_this_slice() {
    let rig = Rig::new();
    let mut reading = read_reading(rig.workflow, rig.alice, ID);
    reading.state = ReadingState::Failed {
        why: "no read came".into(),
        at: Ts::from_millis(2),
    };
    assert!(matches!(
        rig.store.insert_read(&reading),
        Err(StoreError::Invalid(_))
    ));
    assert_eq!(rig.store.reading(rig.alice, &reading.id).unwrap(), None);
}

#[test]
fn the_reading_and_its_gates_land_together() {
    let rig = Rig::new();
    let reading = read_reading(rig.workflow, rig.alice, ID);
    assert_eq!(rig.gates(), None);
    assert_eq!(rig.store.hold_read(&reading).unwrap(), Stored::Written);
    let gates = rig.gates().unwrap();
    assert_eq!(gates.docs().len(), 1);
    assert_eq!(gates.made_from(), Some(&PromptHash::of(SOURCE)));
    assert_eq!(
        rig.store.workflow_reading(rig.alice, rig.workflow).unwrap(),
        Some(reading.id)
    );
}

#[test]
fn a_failed_second_statement_leaves_neither_the_reading_nor_the_gates() {
    let rig = Rig::new();
    rig.break_workflow_updates();
    let reading = read_reading(rig.workflow, rig.alice, ID);
    let err = rig.store.hold_read(&reading).unwrap_err();
    assert!(matches!(err, StoreError::Db(_)), "{err:?}");
    assert_eq!(rig.store.reading(rig.alice, &reading.id).unwrap(), None);
    assert_eq!(rig.gates(), None);
}

#[test]
fn a_workflow_with_no_reading_reads_back_none() {
    let rig = Rig::new();
    assert_eq!(
        rig.store.workflow_reading(rig.alice, rig.workflow).unwrap(),
        None
    );
    assert_eq!(
        rig.store
            .workflow_reading(rig.alice, WorkflowId::new(999))
            .unwrap(),
        None
    );
}

#[test]
fn one_persons_reading_is_not_read_through_another_persons_workflow() {
    let rig = Rig::new();
    let reading = read_reading(rig.workflow, rig.alice, ID);
    assert_eq!(rig.store.hold_read(&reading).unwrap(), Stored::Written);

    assert_eq!(rig.store.reading(rig.bob, &reading.id).unwrap(), None);
    assert_eq!(
        rig.store.workflow_reading(rig.bob, rig.workflow).unwrap(),
        None
    );

    let theirs = read_reading(rig.workflow, rig.bob, OTHER);
    assert_eq!(rig.store.insert_read(&theirs).unwrap(), Stored::NoWorkflow);
    assert_eq!(rig.store.hold_read(&theirs).unwrap(), Stored::NoWorkflow);
    assert_eq!(rig.store.reading(rig.bob, &theirs.id).unwrap(), None);
    assert_eq!(
        rig.store.workflow_reading(rig.alice, rig.workflow).unwrap(),
        Some(reading.id)
    );
}

#[test]
fn a_row_written_here_parses_back_with_its_phrases_and_effects() {
    let rig = Rig::new();
    let reading = read_reading(rig.workflow, rig.alice, ID);
    rig.store.hold_read(&reading).unwrap();
    let back = rig.store.reading(rig.alice, &reading.id).unwrap().unwrap();
    assert_eq!(back, reading);
    let ReadingState::Read(read) = back.state else {
        unreachable!("a read reading");
    };
    assert_eq!(read.phrases().len(), 2);
    assert_eq!(read.effects().len(), 2);
    assert_eq!(read.reader().unwrap().harness, "claude");
}
