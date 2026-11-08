use moor_core::{BoxId, StartReq};

use super::relaunch::{gone, started};
use super::req;
use crate::ops::start::run;
use crate::store::db::DbExt as _;
use crate::testing::agent;
use moor_schema::tables::{Events, Launches, Sessions};
use sea_query::{Asterisk, Expr, ExprTrait as _, Order, Query};

/// Each Session's ordinal, state and the launch rows linked to it, by ordinal.
fn sessions(w: &super::World, id: BoxId) -> Vec<(i64, String, i64)> {
    let store = w.cx.store().unwrap();
    let conn = store.reader().unwrap();
    let launches = Query::select()
        .expr(Expr::col(Asterisk).count())
        .from(Launches::Table)
        .and_where(
            Expr::col((Launches::Table, Launches::Session)).equals((Sessions::Table, Sessions::Id)),
        )
        .take();
    conn.rows(
        Query::select()
            .columns([Sessions::Ordinal, Sessions::State])
            .expr(crate::store::db::sub(launches))
            .from(Sessions::Table)
            .and_where(Expr::col(Sessions::Box).eq(id.get()))
            .order_by(Sessions::Ordinal, Order::Asc),
        |r| Ok::<_, crate::store::db::Error>((r.get(0)?, r.get(1)?, r.get(2)?)),
    )
    .unwrap()
    .collect::<Result<_, _>>()
    .unwrap()
}

#[test]
fn a_first_launch_uses_the_only_session() {
    let (w, _engine, id) = started();
    assert_eq!(sessions(&w, id), [(1, "working".to_owned(), 1)]);
}

#[test]
fn an_exited_agent_ends_its_session_and_the_next_start_opens_the_next() {
    let (w, engine, id) = started();
    engine.set_probes("p-1", agent::exited(Some(3)));
    let again = moor_core::StartReq {
        prompt: Some("ship it".into()),
        ..req("demo")
    };
    run(&w.cx, &again).unwrap();
    assert_eq!(
        sessions(&w, id),
        [(1, "ended".to_owned(), 1), (2, "working".to_owned(), 1)]
    );
    let store = w.cx.store().unwrap();
    let causes: Vec<String> = store
        .reader()
        .unwrap()
        .rows(
            Query::select()
                .column(Events::Payload)
                .from(Events::Table)
                .and_where(Expr::col(Events::Box).eq(id.get()))
                .and_where(Expr::col(Events::Kind).eq("session_ended")),
            |r| r.get(0),
        )
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(causes.len(), 1);
    assert!(causes.iter().all(|c| c.contains("exited")), "{causes:?}");
}

#[test]
fn a_relaunch_carries_a_new_nonce() {
    let (w, engine, _) = started();
    gone(&engine);
    let again = StartReq {
        prompt: Some("ship it".into()),
        ..req("demo")
    };
    run(&w.cx, &again).unwrap();
    let [first, second] = engine.launches().try_into().unwrap();
    assert_ne!(first.nonce, second.nonce);
}
