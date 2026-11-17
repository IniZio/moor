use moor_core::domain::egress::{EgressPath, EgressRow};

use super::{helper_script, request, set_elements, Watch};

/// tinyproxy's config as the egress lines write it, sending GitHub and Anthropic to the Proxy.
const CONF: &str = "Port 41000\nListen 127.0.0.1\nDisableViaHeader Yes\nLogLevel Connect\n\
MaxClients 1024\nupstream http 127.0.0.1:41001 \".githubusercontent.com\"\n\
upstream http 127.0.0.1:41001 \"api.anthropic.com\"\nupstream http 127.0.0.1:41001 \"github.com\"\n\
PidFile \"/home/agent/.moor/egress.pid\"\nLogFile \"/home/agent/.moor/egress-proxy.log\"\n";

/// A tinyproxy log line at `LogLevel Connect` for a request of `what`.
fn line(what: &str) -> String {
    format!("CONNECT   Nov 17 21:27:58.855 [10]: Request (file descriptor 2): {what} HTTP/1.1\n")
}

fn row(host: &str, port: u16, path: EgressPath, count: u64) -> (String, u16, EgressPath, u64) {
    (host.to_owned(), port, path, count)
}

fn seen(w: &Watch) -> Vec<(String, u16, EgressPath, u64)> {
    w.rows()
        .into_iter()
        .map(|r| (r.host, r.port, r.path, r.count))
        .collect()
}

#[test]
fn a_curl_through_the_proxy_is_recorded_as_proxied_and_a_credential_host_as_credential() {
    let mut w = Watch::default();
    let log = [
        "CONNECT   Nov 17 21:27:58.850 [10]: Connect (file descriptor 2): 127.0.0.1\n".to_owned(),
        line("CONNECT example.com:443"),
        "CONNECT   Nov 17 21:27:58.967 [10]: Established connection to host \"example.com\" using file descriptor 3.\n".to_owned(),
        line("CONNECT api.anthropic.com:443"),
        line("CONNECT objects.githubusercontent.com:443"),
        line("GET http://deb.debian.org/debian/dists/bookworm/InRelease?token=s3cret"),
        line("GET http://Example.NET:8080/"),
    ]
    .concat();
    w.take_log(CONF, &log, 5);
    assert_eq!(
        seen(&w),
        [
            row("api.anthropic.com", 443, EgressPath::Credential, 1),
            row("deb.debian.org", 80, EgressPath::Proxied, 1),
            row("example.com", 443, EgressPath::Proxied, 1),
            row("example.net", 8080, EgressPath::Proxied, 1),
            row(
                "objects.githubusercontent.com",
                443,
                EgressPath::Credential,
                1
            ),
        ]
    );
    assert!(w.rows().iter().all(|r| !r.host.contains("token")));
}

#[test]
fn a_host_requested_again_counts_on_one_row() {
    let mut w = Watch::default();
    let log = [line("CONNECT pypi.org:443"), line("CONNECT pypi.org:443")].concat();
    w.take_log(CONF, &log, 5);
    w.take_log(CONF, &line("CONNECT pypi.org:443"), 9);
    assert_eq!(seen(&w), [row("pypi.org", 443, EgressPath::Proxied, 3)]);
    assert_eq!(w.rows().first().map(|r| r.first_seen), Some(5));
}

#[test]
fn a_line_still_being_written_waits_for_the_next_read() {
    let mut w = Watch::default();
    let whole = line("CONNECT pypi.org:443");
    let half = "CONNECT   Nov 17 21:27:58.855 [10]: Requ";
    let used = w.take_log(CONF, &format!("{whole}{half}"), 5);
    assert_eq!(used, whole.len());
    assert_eq!(seen(&w), [row("pypi.org", 443, EgressPath::Proxied, 1)]);
}

#[test]
fn other_lines_name_no_host() {
    assert_eq!(
        request("INFO ... opensock: opening connection to example.com:443"),
        None
    );
    assert_eq!(request(&line("GET /relative/path")), None);
    assert_eq!(
        request(&line("CONNECT [2606:4700::1]:443")),
        Some(("2606:4700::1".to_owned(), 443))
    );
}

/// `nft -j list set` output with one element per `(address, port, packets)`.
fn listed(elems: &[(&str, u16, u64)]) -> String {
    let elem: Vec<serde_json::Value> = elems
        .iter()
        .map(|(a, p, n)| {
            serde_json::json!({"elem": {"val": {"concat": [a, p]}, "counter": {"packets": n, "bytes": 60}}})
        })
        .collect();
    let doc = serde_json::json!({"nftables": [
        {"metainfo": {"version": "1.0.6"}},
        {"set": {"family": "inet", "name": "seen4", "table": "moor_egress", "elem": elem}}
    ]});
    format!("{doc}\n")
}

#[test]
fn a_direct_connection_is_recorded_as_direct_and_counted_once_per_connection() {
    let mut w = Watch::default();
    let sets = set_elements(&listed(&[("93.184.215.14", 443, 1), ("8.8.8.8", 53, 2)]));
    w.take_sets(&sets, 5);
    assert_eq!(
        seen(&w),
        [
            row("8.8.8.8", 53, EgressPath::Direct, 2),
            row("93.184.215.14", 443, EgressPath::Direct, 1),
        ]
    );
    let again = set_elements(&listed(&[("93.184.215.14", 443, 3), ("8.8.8.8", 53, 2)]));
    w.take_sets(&again, 9);
    assert_eq!(
        seen(&w),
        [
            row("8.8.8.8", 53, EgressPath::Direct, 2),
            row("93.184.215.14", 443, EgressPath::Direct, 3),
        ]
    );
}

#[test]
fn counters_that_restart_in_a_new_network_namespace_still_add_up() {
    let mut w = Watch::default();
    w.take_sets(&[("1.1.1.1".to_owned(), 443, 5)], 5);
    w.take_sets(&[("1.1.1.1".to_owned(), 443, 2)], 9);
    assert_eq!(seen(&w), [row("1.1.1.1", 443, EgressPath::Direct, 7)]);
}

#[test]
fn a_resumed_tally_does_not_count_what_its_file_already_holds() {
    let held = vec![EgressRow {
        host: "pypi.org".to_owned(),
        port: 443,
        path: EgressPath::Proxied,
        first_seen: 1,
        count: 4,
    }];
    let mut w = Watch::resumed(held);
    let log = [
        line("CONNECT pypi.org:443"),
        line("CONNECT files.pythonhosted.org:443"),
    ]
    .concat();
    w.take_log(CONF, &log, 5);
    assert_eq!(
        seen(&w),
        [
            row("files.pythonhosted.org", 443, EgressPath::Proxied, 1),
            row("pypi.org", 443, EgressPath::Proxied, 4),
        ]
    );
}

#[test]
fn the_helper_never_drops_or_rejects_a_packet() {
    let script = helper_script();
    for word in ["drop", "reject", "policy drop", "flush ruleset"] {
        assert!(!script.contains(word), "{word} in {script}");
    }
}
