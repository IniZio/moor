use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use moor_core::seam::clock::Ts;
use sea_query::{Expr, ExprTrait as _, Query};

use super::types::numeric;
use super::{Pool, Target, Value};
use crate::store::db::testing::{open_conn, postgres_target};
use crate::store::db::{Conn, Db, DbExt as _};
use crate::store::Store;
use crate::testing::clock::FakeClock;

/// A binary numeric of base-10000 `digits` at `weight`.
fn raw(weight: i16, negative: bool, scale: u16, digits: &[i16]) -> Vec<u8> {
    let sign: u16 = if negative { 0x4000 } else { 0 };
    let len = i16::try_from(digits.len()).unwrap();
    let mut out = Vec::new();
    for w in [len, weight, sign.cast_signed(), scale.cast_signed()]
        .iter()
        .chain(digits)
    {
        out.extend(w.to_be_bytes());
    }
    out
}

#[test]
fn a_whole_numeric_reads_as_an_integer() {
    assert_eq!(numeric(&raw(0, false, 0, &[])).unwrap(), Value::Integer(0));
    assert_eq!(
        numeric(&raw(0, false, 0, &[42])).unwrap(),
        Value::Integer(42)
    );
    assert_eq!(
        numeric(&raw(1, true, 0, &[12, 3456])).unwrap(),
        Value::Integer(-123_456)
    );
    assert_eq!(
        numeric(&raw(2, false, 0, &[7])).unwrap(),
        Value::Integer(700_000_000)
    );
    assert_eq!(
        numeric(&raw(4, false, 0, &[922, 3372, 368, 5477, 5807])).unwrap(),
        Value::Integer(i64::MAX)
    );
}

#[test]
fn a_fractional_or_huge_numeric_reads_as_a_real() {
    assert_eq!(
        numeric(&raw(0, false, 1, &[1, 5000])).unwrap(),
        Value::Real(1.5)
    );
    assert_eq!(
        numeric(&raw(-1, true, 2, &[2500])).unwrap(),
        Value::Real(-0.25)
    );
    assert_eq!(numeric(&raw(5, false, 0, &[1])).unwrap(), Value::Real(1e20));
}

#[test]
fn a_nan_or_short_numeric_is_refused() {
    let mut nan = raw(0, false, 0, &[]);
    if let Some(sign) = nan.get_mut(4) {
        *sign = 0xC0;
    }
    assert!(numeric(&nan).is_err());
    let short = raw(0, false, 0, &[1]);
    assert!(numeric(short.get(..9).unwrap()).is_err());
}

/// The server process behind `conn`.
fn backend_pid(conn: &dyn Db) -> i64 {
    conn.row(Query::select().expr(Expr::cust("pg_backend_pid()")), |r| {
        r.get(0)
    })
    .unwrap()
}

#[test]
fn a_dropped_connection_fails_its_next_call_and_the_one_after_reconnects() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("moor.db");
    let Some(target) = postgres_target(&path) else {
        return;
    };
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000)));
    let store = Store::open_postgres(&target, 4, dir.path(), clock).unwrap();
    let names = |store: &Store| {
        store.reader().and_then(|r| {
            Ok(r.rows(
                Query::select()
                    .column("k")
                    .from("meta")
                    .and_where(Expr::col("k").eq("x")),
                |r| r.get::<_, String>(0),
            )?
            .collect::<Result<Vec<_>, _>>()?)
        })
    };
    let writer = backend_pid(&*store.writer());
    let reader = backend_pid(&store.reader().unwrap());
    let Conn::Postgres(admin) = open_conn(&path) else {
        unreachable!("a Postgres connection")
    };
    for pid in [writer, reader] {
        admin
            .execute_batch(&format!("SELECT pg_terminate_backend({pid}, 5000)"))
            .unwrap();
    }
    let put = || {
        store.writer().run(
            Query::insert()
                .into_table("meta")
                .columns(["k", "v"])
                .values_panic(["x".into(), "1".into()]),
        )
    };
    assert!(put().is_err(), "a write on a dropped connection");
    put().unwrap();
    assert!(names(&store).is_err(), "a read on a dropped connection");
    assert_eq!(names(&store).unwrap(), ["x"]);
}

/// A pool of at most `cap` read connections, all named `name` to the server, with the URL's
/// admin connection that counts them.
fn capped_pool(name: &str, cap: usize, wait: Duration) -> Option<(Pool, postgres::Client)> {
    let url = std::env::var("MOOR_TEST_POSTGRES_URL").ok()?;
    let app = format!("moor_cap_{}_{name}", std::process::id());
    let sep = if url.contains('?') { '&' } else { '?' };
    let target = Target::parse(&format!("{url}{sep}application_name={app}")).unwrap();
    let admin = postgres::Client::connect(&url, postgres::NoTls).unwrap();
    Some((Pool::waiting(target, cap, wait), admin))
}

/// The connections open under application `name` of this test process.
fn open_under(admin: &mut postgres::Client, name: &str) -> i64 {
    let app = format!("moor_cap_{}_{name}", std::process::id());
    admin
        .query_one(
            "SELECT count(*) FROM pg_stat_activity WHERE application_name = $1",
            &[&app],
        )
        .unwrap()
        .get(0)
}

fn select_one(r: &impl Db) -> Result<(), super::Error> {
    r.row(Query::select().expr(Expr::cust("1")), |r| {
        r.get::<_, i64>(0)
    })
    .map(|_| ())
}

#[test]
fn a_read_past_the_cap_waits_then_fails_naming_it_and_succeeds_once_one_is_given_back() {
    let Some((pool, mut admin)) = capped_pool("past", 2, Duration::from_millis(300)) else {
        return;
    };
    let first = pool.take().unwrap();
    let second = pool.take().unwrap();
    select_one(&first).unwrap();
    select_one(&second).unwrap();
    let refused = pool.take().err().expect("a third read past the cap of 2");
    assert!(
        refused.to_string().contains("MOOR_DATABASE_READERS"),
        "{refused}"
    );
    assert_eq!(open_under(&mut admin, "past"), 2);
    drop(first);
    select_one(&pool.take().unwrap()).unwrap();
    assert_eq!(open_under(&mut admin, "past"), 2);
    drop(second);
}

#[test]
fn concurrent_reads_never_open_more_connections_than_the_cap() {
    let Some((pool, mut admin)) = capped_pool("burst", 3, Duration::from_secs(30)) else {
        return;
    };
    let most = AtomicI64::new(0);
    let done = AtomicBool::new(false);
    std::thread::scope(|s| {
        s.spawn(|| {
            let mut admin = {
                let url = std::env::var("MOOR_TEST_POSTGRES_URL").unwrap();
                postgres::Client::connect(&url, postgres::NoTls).unwrap()
            };
            while !done.load(Ordering::Relaxed) {
                most.fetch_max(open_under(&mut admin, "burst"), Ordering::Relaxed);
            }
        });
        let readers: Vec<_> = (0..16)
            .map(|_| {
                s.spawn(|| {
                    for _ in 0..10 {
                        let r = pool.take().unwrap();
                        select_one(&r).unwrap();
                        std::thread::sleep(Duration::from_millis(5));
                    }
                })
            })
            .collect();
        for r in readers {
            r.join().unwrap();
        }
        done.store(true, Ordering::Relaxed);
    });
    let most = most.load(Ordering::Relaxed);
    assert!((1..=3).contains(&most), "{most} connections for a cap of 3");
    assert!(open_under(&mut admin, "burst") <= 3);
}
