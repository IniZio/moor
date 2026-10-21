//! An upstream that refuses the connection, and what the Proxy records.

use std::time::Duration;

use super::e2e::{get, kinds, rig};
use crate::proxy::nono::audit::{self, BoxSink, Withheld};

#[test]
fn an_upstream_refusing_the_connection_is_recorded_as_a_connect_failure() {
    let mut rig = rig();
    rig.up.close();
    assert_eq!(get(&rig.client(), &rig.url("/ok")), Ok(502));
    assert!(rig.up.seen().is_empty());

    let (store, box_id, log) = rig.store();
    let sink = BoxSink {
        log: &log,
        store: &store,
        box_id,
    };
    let watch = Withheld::default();
    for _ in 0..100 {
        audit::apply(&rig.proxy().drain_audit(), &watch, &sink);
        if kinds(&store, box_id).iter().any(|k| k == "egress_refused") {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    audit::apply(&rig.proxy().drain_audit(), &watch, &sink);

    let events = store.events(box_id, 100).unwrap();
    let refused: Vec<_> = events
        .iter()
        .filter(|e| e.kind == "egress_refused")
        .collect();
    assert_eq!(refused.len(), 1, "{events:?}");
    let only = refused.first().unwrap();
    let field = |name: &str| only.payload.get(name).and_then(|v| v.as_str());
    assert_eq!(field("category"), Some("upstream_connect_failed"));
    assert_eq!(field("host"), Some("localhost"));
    assert_eq!(field("rule"), Some("hdr"));
    let found = kinds(&store, box_id);
    assert!(
        !found.iter().any(|k| k.starts_with("proxy_credential_")),
        "{found:?}"
    );
}
