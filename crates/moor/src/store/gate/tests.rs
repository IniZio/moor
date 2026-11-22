//! A Box's gates are kept with the Box, and its lines are appended under a key that decides a
//! race.

use moor_core::domain::decision::{Decision, NewBox, SpecFields};
use moor_core::gate::{GateDoc, GateLine, Gates};
use moor_core::seam::clock::Ts;
use moor_core::workflow::StepId;
use moor_core::BoxName;
use moor_schema::tables::BoxGateLines;

use crate::store::tests::{new_box, open};
use crate::store::StoreError;

fn proof() -> GateDoc {
    GateDoc {
        name: "proof".to_owned(),
        evidence: vec!["proof.md".to_owned()],
        checks: Vec::new(),
        approval: None,
        max_tries: None,
    }
}

fn reminded(try_n: u8) -> GateLine {
    GateLine::Reminded {
        gate: StepId::parse("proof").unwrap(),
        try_n,
    }
}

#[test]
fn gates_are_stored_with_the_box_and_a_box_without_has_none() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let json = serde_json::to_string(&Gates::parse(&[proof()], &[]).unwrap().doc()).unwrap();
    let with = store
        .apply(Decision::insert(NewBox {
            name: BoxName::parse_allow_bare("a").unwrap(),
            repo: None,
            spec: SpecFields {
                gates: Some(json),
                ..SpecFields::default()
            },
        }))
        .unwrap()
        .box_id;
    let without = new_box(&store, "b");
    assert_eq!(
        store.box_gates(with).unwrap(),
        Some(Gates::parse(&[proof()], &[]).unwrap())
    );
    assert_eq!(store.box_gates(without).unwrap(), None);
}

#[test]
fn of_two_appends_of_one_line_number_one_fails() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let id = new_box(&store, "a");
    let at = Ts::from_millis(5);
    store.append_gate_line(id, 1, at, &reminded(1)).unwrap();
    let err = store.append_gate_line(id, 1, at, &reminded(2)).unwrap_err();
    let taken = matches!(&err, StoreError::Db(db) if db
        .unique_key()
        .is_some_and(|k| k.is(&BoxGateLines::Table, &[BoxGateLines::Box, BoxGateLines::N])));
    assert!(taken, "{err:?}");
    store.append_gate_line(id, 2, at, &reminded(2)).unwrap();
    assert_eq!(
        store.gate_lines(id).unwrap(),
        [reminded(1), reminded(2)],
        "the loser left no line"
    );
    let other = new_box(&store, "b");
    store.append_gate_line(other, 1, at, &reminded(1)).unwrap();
    assert_eq!(store.gate_lines(other).unwrap(), [reminded(1)]);
}

#[test]
fn a_line_of_a_box_that_does_not_exist_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let missing = moor_core::BoxId::new(99);
    let err = store
        .append_gate_line(missing, 1, Ts::from_millis(5), &reminded(1))
        .unwrap_err();
    assert!(matches!(err, StoreError::Db(_)), "{err:?}");
}

#[test]
fn the_lines_of_a_box_go_with_the_box() {
    use crate::store::db::DbExt as _;
    use moor_schema::tables::Boxes;
    use sea_query::{Alias, Expr, ExprTrait as _, Query};

    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let (gone, kept) = (new_box(&store, "a"), new_box(&store, "b"));
    for id in [gone, kept] {
        store
            .append_gate_line(id, 1, Ts::from_millis(5), &reminded(1))
            .unwrap();
    }
    let tables = moor_schema::tables::all();
    for (table, columns) in tables.iter().rev() {
        let owned =
            columns.iter().any(|c| c == "box") && !matches!(table.as_str(), "box_gate_lines");
        if owned {
            let rows = Query::delete()
                .from_table(Alias::new(table))
                .and_where(Expr::col(Alias::new("box")).eq(gone.get()))
                .take();
            store.writer().run(rows).unwrap();
        }
    }
    store
        .writer()
        .run(
            Query::delete()
                .from_table(Boxes::Table)
                .and_where(Expr::col(Boxes::Id).eq(gone.get())),
        )
        .unwrap();
    assert_eq!(store.gate_lines(gone).unwrap(), []);
    assert_eq!(store.gate_lines(kept).unwrap(), [reminded(1)]);
}
