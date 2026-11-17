use super::{EgressFlushed, EgressPath, EgressRow, EgressTally, COUNT_EVERY_MS};

fn row(host: &str, port: u16, path: EgressPath, first_seen: u64, count: u64) -> EgressRow {
    EgressRow {
        host: host.to_owned(),
        port,
        path,
        first_seen,
        count,
    }
}

#[test]
fn a_host_seen_twice_is_one_row_with_both_counts_and_the_first_time() {
    let mut tally = EgressTally::default();
    tally.add("example.com", 443, EgressPath::Proxied, 2_000, 1);
    tally.add("example.com", 443, EgressPath::Proxied, 1_000, 2);
    assert_eq!(
        tally.rows(),
        [row("example.com", 443, EgressPath::Proxied, 1_000, 3)]
    );
}

#[test]
fn the_same_host_by_another_path_or_port_is_its_own_row() {
    let tally = EgressTally::of(vec![
        row("example.com", 443, EgressPath::Proxied, 1, 1),
        row("example.com", 443, EgressPath::Direct, 1, 1),
        row("example.com", 80, EgressPath::Proxied, 1, 1),
    ]);
    assert_eq!(tally.rows().len(), 3);
}

#[test]
fn the_rows_survive_the_file_they_are_kept_in() {
    let rows = vec![row("1.2.3.4", 53, EgressPath::Direct, 7, 2)];
    let text = serde_json::to_string(&rows).unwrap();
    assert!(text.contains(r#""path":"direct""#), "{text}");
    let back: Vec<EgressRow> = serde_json::from_str(&text).unwrap();
    assert_eq!(back, rows);
}

#[test]
fn a_new_row_is_written_once() {
    let mut flushed = EgressFlushed::default();
    let rows = [row("example.com", 443, EgressPath::Proxied, 1, 1)];
    assert_eq!(flushed.take(&rows, 10), rows);
    assert!(flushed.take(&rows, 20).is_empty());
}

#[test]
fn a_changed_count_rides_with_the_next_new_row() {
    let mut flushed = EgressFlushed::default();
    let a1 = row("a.example", 443, EgressPath::Proxied, 1, 1);
    flushed.take(std::slice::from_ref(&a1), 10);
    let a5 = row("a.example", 443, EgressPath::Proxied, 1, 5);
    assert!(flushed.take(std::slice::from_ref(&a5), 20).is_empty());
    let b = row("b.example", 443, EgressPath::Direct, 15, 1);
    assert_eq!(flushed.take(&[a5.clone(), b.clone()], 30), [a5, b]);
}

#[test]
fn a_changed_count_alone_waits_for_the_count_interval() {
    let mut flushed = EgressFlushed::default();
    flushed.take(&[row("a.example", 443, EgressPath::Proxied, 1, 1)], 10);
    let a2 = [row("a.example", 443, EgressPath::Proxied, 1, 2)];
    assert!(flushed.take(&a2, 10 + COUNT_EVERY_MS - 1).is_empty());
    assert_eq!(flushed.take(&a2, 10 + COUNT_EVERY_MS), a2);
}
