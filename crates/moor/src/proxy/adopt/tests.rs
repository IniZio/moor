//! Adopting or restarting recorded Proxy supervisors when the controller starts.

use std::sync::Arc;
use std::time::Duration;

use moor_core::domain::decision::{Decision, NewBox};
use moor_core::seam::clock::{Clock as _, Ts};
use moor_core::seam::proxy_host::{ProxyHost as _, ProxySpec, ProxyStatus};
use moor_core::{BoxId, BoxName};

use moor_schema::tables::Boxes;
use sea_query::{Expr, ExprTrait as _, Query, UpdateStatement};

use super::reconcile;
use crate::proxy::detached::{DetachedSupervisor, LEASE};
use crate::proxy::ledger::LEDGER;
use crate::store::db::{Conn, DbExt as _};
use crate::store::{ReadyProc, Store, STORE_FILE};
use crate::testing::clock::FakeClock;
use crate::testing::proc::ScriptedProc;
use crate::testing::proxy_host::stored_ca;
use crate::testing::rng::FakeRng;

const PID: u32 = 4242;
const BOOT: &str = "boot-a";

struct Fx {
    dir: tempfile::TempDir,
    proc: Arc<ScriptedProc>,
    clock: Arc<FakeClock>,
    store: Store,
    host: DetachedSupervisor,
    id: BoxId,
}

/// A Box with a ready machine and a recorded Proxy whose supervisor is `PID` in boot `BOOT`.
fn fx() -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let proc = Arc::new(ScriptedProc::new());
    proc.with_boot_id(BOOT);
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1000)));
    let rng = Arc::new(FakeRng::default());
    let store = Store::open(&dir.path().join(STORE_FILE), clock.clone()).unwrap();
    let id = store
        .apply(Decision::insert(NewBox {
            name: BoxName::parse_allow_bare("t").unwrap(),
            repo: None,
        }))
        .unwrap()
        .box_id;
    let host = DetachedSupervisor::new(
        proc.clone(),
        clock.clone(),
        rng,
        Some(dir.path().to_path_buf()),
    );
    let f = Fx {
        dir,
        proc,
        clock,
        store,
        host,
        id,
    };
    f.exec(
        Query::update()
            .table(Boxes::Table)
            .values([
                (Boxes::Provider, "crabbox".into()),
                (Boxes::DriverState, "ready".into()),
                (Boxes::DriverUpdatedAt, 0_i64.into()),
            ])
            .and_where(Expr::col(Boxes::Id).eq(id.get()))
            .take(),
    );
    let state = f.dir.path().to_path_buf();
    let shared = f.clock.clone();
    f.proc.on_daemon(move |d| {
        let store = Store::open(&state.join(STORE_FILE), shared.clone()).unwrap();
        let box_id = BoxId::new(d.call.args.get(2).unwrap().parse().unwrap());
        store.claim_lease(box_id, "new-supervisor", LEASE).unwrap();
        store.keep_proxy_ca(box_id, &stored_ca(box_id)).unwrap();
        let box_port = store.proxy(box_id).unwrap().unwrap().box_port;
        let proc = ReadyProc {
            pid: d.pid,
            handle: "h",
            boot_id: Some(BOOT),
        };
        let _ = store
            .mark_ready(box_id, "new-supervisor", box_port, proc)
            .unwrap();
    });
    f
}

impl Fx {
    fn spec(&self) -> ProxySpec {
        ProxySpec {
            box_id: self.id,
            box_handle: None,
            other_repos: Vec::new(),
            box_repo: None,
            rules: Vec::new(),
            user: moor_core::user::UserId::new(1),
        }
    }

    fn exec(&self, update: UpdateStatement) {
        Conn::open_file(&self.dir.path().join(STORE_FILE))
            .unwrap()
            .run(update)
            .unwrap();
    }

    /// Records a ready Proxy held by a supervisor with `PID` written in `boot`.
    fn record(&self, boot: &str) -> u16 {
        let _ = self
            .store
            .reserve(self.id, &FakeRng::default(), LEASE)
            .unwrap();
        if self.store.proxy_spec(self.id).unwrap().is_none() {
            self.store.set_proxy_spec(self.id, &self.spec()).unwrap();
        }
        self.store
            .set_proxy_key(self.id, b"old-key", LEDGER)
            .unwrap();
        self.store.claim_lease(self.id, "old", LEASE).unwrap();
        let box_port = self.store.proxy(self.id).unwrap().unwrap().box_port;
        let proc = ReadyProc {
            pid: PID,
            handle: "old-handle",
            boot_id: Some(boot),
        };
        let _ = self
            .store
            .mark_ready(self.id, "old", box_port, proc)
            .unwrap();
        box_port
    }

    fn settle(&self) -> Vec<String> {
        reconcile(&self.host, self.proc.as_ref(), &self.store)
    }

    fn row(&self) -> crate::store::ProxyRow {
        self.store.proxy(self.id).unwrap().unwrap()
    }
}

#[test]
fn a_live_group_leader_in_this_boot_is_adopted_untouched() {
    let f = fx();
    let port = f.record(BOOT);

    assert_eq!(f.settle(), Vec::<String>::new());

    assert!(f.proc.daemons().is_empty());
    assert!(f.proc.signals().is_empty());
    let row = f.row();
    assert_eq!(row.pid, Some(PID));
    assert_eq!(row.box_port, port);
}

#[test]
fn a_dead_pid_is_restarted_on_the_same_box_port() {
    let f = fx();
    let port = f.record(BOOT);
    f.proc.dead(PID);

    assert_eq!(f.settle(), Vec::<String>::new());

    assert_eq!(f.proc.daemons().len(), 1);
    assert!(f.proc.signals().is_empty());
    let row = f.row();
    assert_eq!(row.box_port, port);
    assert!(row.ready);
    assert_ne!(row.pid, Some(PID));
}

#[test]
fn a_row_from_another_boot_is_restarted_without_a_signal() {
    let f = fx();
    let port = f.record("boot-before-reboot");

    assert_eq!(f.settle(), Vec::<String>::new());

    assert_eq!(f.proc.daemons().len(), 1);
    assert!(f.proc.signals().is_empty());
    assert_eq!(f.row().box_port, port);
    assert_eq!(f.row().boot_id.as_deref(), Some(BOOT));
}

#[test]
fn a_live_pid_that_is_not_a_group_leader_is_never_signalled() {
    let f = fx();
    let port = f.record(BOOT);
    f.proc.follower(PID);

    assert_eq!(f.settle(), Vec::<String>::new());

    assert!(f.proc.signals().is_empty());
    assert_eq!(f.proc.daemons().len(), 1);
    assert_eq!(f.row().box_port, port);
    assert_ne!(f.row().pid, Some(PID));
}

#[test]
fn a_stale_lease_is_restarted_on_the_same_box_port() {
    let f = fx();
    let port = f.record(BOOT);
    f.clock.sleep(LEASE + Duration::from_secs(1));

    assert_eq!(f.settle(), Vec::<String>::new());

    assert_eq!(f.proc.daemons().len(), 1);
    assert_eq!(f.row().box_port, port);
    assert_eq!(
        f.host.status(f.id).unwrap(),
        ProxyStatus::Ready { box_port: port }
    );
}

#[test]
fn a_box_without_a_usable_box_has_its_proxy_stopped_not_restarted() {
    let f = fx();
    f.record(BOOT);
    f.clock.sleep(LEASE + Duration::from_secs(1));
    f.exec(
        Query::update()
            .table(Boxes::Table)
            .value(Boxes::DriverState, "deleted")
            .take(),
    );

    assert_eq!(f.settle(), Vec::<String>::new());

    assert!(f.proc.daemons().is_empty());
    assert!(f.store.proxy(f.id).unwrap().is_none());
}

#[test]
fn a_missing_spec_is_reported_and_leaves_the_row_stale() {
    let f = fx();
    let port = f.record(BOOT);
    f.proc.dead(PID);
    f.exec(
        Query::update()
            .table(moor_schema::tables::Proxy::Table)
            .value(moor_schema::tables::Proxy::Spec, Option::<String>::None)
            .take(),
    );

    let errors = f.settle();

    assert_eq!(errors.len(), 1);
    assert!(errors.iter().all(|e| e.contains("without its spec")));
    assert!(f.proc.daemons().is_empty());
    assert_eq!(f.row().box_port, port);
    assert_eq!(f.host.status(f.id).unwrap(), ProxyStatus::Stale);
}

#[test]
fn a_restarted_proxy_keeps_acting_as_the_user_its_spec_names() {
    let f = fx();
    let mut spec = f.spec();
    spec.user = moor_core::user::UserId::new(7);
    f.record(BOOT);
    f.store.set_proxy_spec(f.id, &spec).unwrap();
    f.proc.dead(PID);

    assert_eq!(f.settle(), Vec::<String>::new());

    assert_eq!(f.proc.daemons().len(), 1);
    assert_eq!(f.store.proxy_spec(f.id).unwrap(), Some(spec));
}

/// Records a ready Proxy whose supervisor was started to speak `ledger`, or before the ledger
/// when `None`, and lets it give up its lease when signalled.
fn record_speaking(f: &Fx, ledger: Option<u32>) {
    f.record(BOOT);
    let column = moor_schema::tables::Proxy::Ledger;
    f.exec(
        Query::update()
            .table(moor_schema::tables::Proxy::Table)
            .value(column, ledger.map(i64::from))
            .take(),
    );
    let state = f.dir.path().to_path_buf();
    let (shared, id) = (f.clock.clone(), f.id);
    f.proc.on_signal(move |pid, _| {
        if pid == PID {
            let store = Store::open(&state.join(STORE_FILE), shared.clone()).unwrap();
            store.release(id, "old").unwrap();
        }
    });
}

#[test]
fn a_live_supervisor_from_before_the_ledger_is_replaced_on_the_same_box_port() {
    let f = fx();
    record_speaking(&f, None);
    let port = f.row().box_port;

    assert_eq!(f.settle(), Vec::<String>::new());

    assert_eq!(f.proc.signals().first().map(|s| s.0), Some(PID));
    assert_eq!(f.proc.daemons().len(), 1);
    let row = f.row();
    assert_eq!(row.box_port, port);
    assert_eq!(row.ledger, Some(LEDGER));
    assert_ne!(row.pid, Some(PID));
}

#[test]
fn a_live_supervisor_at_another_ledger_protocol_is_replaced() {
    let f = fx();
    record_speaking(&f, Some(LEDGER + 1));

    assert_eq!(f.settle(), Vec::<String>::new());

    assert_eq!(f.proc.signals().first().map(|s| s.0), Some(PID));
    assert_eq!(f.proc.daemons().len(), 1);
    assert_eq!(f.row().ledger, Some(LEDGER));
}

#[test]
fn a_replaced_supervisor_that_outlasts_the_wait_is_reported_and_not_doubled() {
    let f = fx();
    f.record(BOOT);
    f.exec(
        Query::update()
            .table(moor_schema::tables::Proxy::Table)
            .value(moor_schema::tables::Proxy::Ledger, Option::<i64>::None)
            .take(),
    );

    let errors = f.settle();

    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(
        errors.iter().all(|e| e.contains("did not exit")),
        "{errors:?}"
    );
    assert!(f.proc.daemons().is_empty());
}

#[test]
fn a_supervisor_from_before_the_shared_proxy_is_replaced_by_one_that_relays() {
    let f = fx();
    let relay: std::net::SocketAddr = "127.0.0.1:18490".parse().unwrap();
    let f = Fx {
        host: f.host.relaying(Some(relay)),
        ..f
    };
    record_speaking(&f, Some(LEDGER));

    assert_eq!(f.settle(), Vec::<String>::new());

    assert_eq!(f.proc.signals().first().map(|s| s.0), Some(PID));
    let daemons = f.proc.daemons();
    assert_eq!(daemons.len(), 1);
    assert!(daemons
        .first()
        .unwrap()
        .call
        .env
        .contains(&("MOOR_EGRESS_RELAY".to_owned(), "127.0.0.1:18490".to_owned())));
    assert_eq!(f.row().ledger, Some(f.host.started_as()));
    assert_ne!(f.host.started_as(), LEDGER);
}
