//! The withheld and dialled lists a rules read carries.

use moor_core::view::repo_config::{DialledSeen, WithheldSeen};
use serde_json::{json, Value};

use super::seen;

fn withheld(box_id: i64, shape: &str) -> (i64, String, Value) {
    let payload = json!({"host": "api.github.com", "rule": "github-api", "method": "POST",
        "shape": shape, "reason": "no allow entry matches the request"});
    (box_id, "credential_withheld".to_owned(), payload)
}

fn egress(box_id: i64, rows: &Value) -> (i64, String, Value) {
    (box_id, "egress_seen".to_owned(), json!({ "hosts": rows }))
}

#[test]
fn a_withheld_request_counts_each_box_that_reported_it_once() {
    let comments = "POST /repos/acme/web/issues/482/comments";
    let events = [
        withheld(1, comments),
        withheld(2, comments),
        withheld(2, comments),
        withheld(1, "PATCH /repos/acme/web/git/refs/heads/tmp"),
    ];
    let lists = seen(&events, 7);
    assert_eq!(lists.days, 7);
    assert_eq!(
        lists.withheld,
        [
            WithheldSeen {
                request: comments.to_owned(),
                host: "api.github.com".to_owned(),
                boxes: 2,
            },
            WithheldSeen {
                request: "PATCH /repos/acme/web/git/refs/heads/tmp".to_owned(),
                host: "api.github.com".to_owned(),
                boxes: 1,
            },
        ]
    );
}

#[test]
fn dialled_hosts_sum_the_newest_count_of_each_box_and_leave_out_credential_hosts() {
    let npm = |count: u64| json!({"host": "registry.npmjs.org", "port": 443, "path": "proxied", "count": count});
    let events = [
        egress(
            1,
            &json!([npm(10), {"host": "github.com", "port": 443, "path": "credential", "count": 5}]),
        ),
        egress(1, &json!([npm(200)])),
        egress(
            2,
            &json!([npm(12), {"host": "151.101.1.1", "port": 80, "path": "direct", "count": 3}]),
        ),
    ];
    let lists = seen(&events, 7);
    assert_eq!(
        lists.dialled,
        [
            DialledSeen {
                host: "registry.npmjs.org".to_owned(),
                port: 443,
                count: 212,
            },
            DialledSeen {
                host: "151.101.1.1".to_owned(),
                port: 80,
                count: 3,
            },
        ]
    );
    assert!(lists.withheld.is_empty());
}
