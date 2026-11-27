//! The push guard against the command lists git sends.

use gix_packetline::decode;
use gix_packetline::PacketLineRef;

use super::{judge, read_commands, report, Owned, Refused, KEY};

/// An object id.
const A: &str = "1111111111111111111111111111111111111111";
/// Another object id.
const B: &str = "2222222222222222222222222222222222222222";
/// The id a deletion moves a ref to.
const ZERO: &str = "0000000000000000000000000000000000000000";
/// The Box's branch.
const MINE: &str = "moor/12-fix";

/// One pkt-line holding `data`.
fn pkt(data: &str) -> String {
    format!("{:04x}{data}", data.len() + 4)
}

/// The command list git sends for `updates` asking for `caps`, then a pack.
fn push(updates: &[(&str, &str, &str)], caps: &str) -> Vec<u8> {
    let mut out = String::new();
    for (i, (old, new, name)) in updates.iter().enumerate() {
        let line = if i == 0 {
            format!("{old} {new} {name}\0{caps}\n")
        } else {
            format!("{old} {new} {name}\n")
        };
        out.push_str(&pkt(&line));
    }
    out.push_str("0000PACK-bytes-that-stay-unread");
    out.into_bytes()
}

/// The Box that owns [`MINE`].
fn owned() -> Owned {
    Owned::branch(Some(MINE))
}

/// Reads and judges a push.
fn verdict(body: &[u8], owned: &Owned) -> Result<(), Refused> {
    let commands = read_commands(&mut &body[..])?;
    judge(&commands.updates, owned)
}

#[test]
fn a_push_to_the_box_own_branch_passes() {
    let body = push(&[(A, B, "refs/heads/moor/12-fix")], "report-status");
    assert_eq!(verdict(&body, &owned()), Ok(()));
}

#[test]
fn a_new_box_branch_passes() {
    let body = push(&[(ZERO, B, "refs/heads/moor/12-fix")], "report-status");
    assert_eq!(verdict(&body, &owned()), Ok(()));
}

#[test]
fn a_push_to_the_default_branch_is_refused() {
    let body = push(&[(A, B, "refs/heads/main")], "report-status");
    let refused = verdict(&body, &owned()).unwrap_err();
    assert_eq!(refused, Refused::NotOwned("refs/heads/main".to_owned()));
    let message = refused.message(&owned());
    assert!(message.contains("refs/heads/main"), "{message}");
    assert!(message.contains("refs/heads/moor/12-fix"), "{message}");
    assert!(message.contains(&format!("contract: {KEY}")), "{message}");
}

#[test]
fn a_push_to_another_box_branch_or_a_tag_is_refused() {
    for name in [
        "refs/heads/moor/13-other",
        "refs/tags/v1",
        "refs/heads/moor/12-fix-2",
    ] {
        let body = push(&[(A, B, name)], "report-status");
        assert_eq!(
            verdict(&body, &owned()),
            Err(Refused::NotOwned(name.to_owned()))
        );
    }
}

#[test]
fn deleting_even_the_box_own_branch_is_refused() {
    let body = push(&[(A, ZERO, "refs/heads/moor/12-fix")], "report-status");
    assert_eq!(
        verdict(&body, &owned()),
        Err(Refused::Deletes("refs/heads/moor/12-fix".to_owned()))
    );
}

#[test]
fn one_foreign_ref_refuses_the_whole_push() {
    let body = push(
        &[(A, B, "refs/heads/moor/12-fix"), (A, B, "refs/heads/main")],
        "report-status",
    );
    assert_eq!(
        verdict(&body, &owned()),
        Err(Refused::NotOwned("refs/heads/main".to_owned()))
    );
}

#[test]
fn a_box_with_no_branch_owns_nothing() {
    let body = push(&[(A, B, "refs/heads/moor/12-fix")], "report-status");
    let none = Owned::branch(None);
    let refused = verdict(&body, &none).unwrap_err();
    assert!(refused.message(&none).contains("owns no branch"));
    assert_eq!(verdict(&body, &Owned::branch(Some(""))), Err(refused));
}

#[test]
fn the_read_stops_at_the_flush_and_keeps_the_exact_bytes() {
    let body = push(
        &[(A, B, "refs/heads/moor/12-fix")],
        "report-status side-band-64k agent=git/2.45",
    );
    let mut rest = &body[..];
    let commands = read_commands(&mut rest).unwrap();
    assert_eq!(rest, b"PACK-bytes-that-stay-unread");
    assert_eq!(commands.raw, body.get(..body.len() - rest.len()).unwrap());
    assert_eq!(
        commands.caps,
        ["report-status", "side-band-64k", "agent=git/2.45"]
    );
    assert_eq!(commands.updates.len(), 1);
}

#[test]
fn shallow_lines_are_not_ref_updates() {
    let mut body = pkt(&format!("shallow {A}\n")).into_bytes();
    body.extend(push(&[(A, B, "refs/heads/moor/12-fix")], "report-status"));
    assert_eq!(verdict(&body, &owned()), Ok(()));
}

#[test]
fn a_push_moor_cannot_read_is_refused() {
    let signed = format!("{}0000", pkt("push-cert\0report-status\n"));
    let not_an_update = format!("{}0000", pkt("hello there\n"));
    let short_id = format!("{}0000", pkt(&format!("{A} abc refs/heads/x\n")));
    let no_refs = format!("{}0000", pkt(&format!("{A} {B} main\n")));
    let truncated = pkt(&format!("{A} {B} refs/heads/moor/12-fix\n"));
    let delimiter = format!("{}0001", pkt(&format!("{A} {B} refs/heads/moor/12-fix\n")));
    for body in [
        signed,
        not_an_update,
        short_id,
        no_refs,
        truncated,
        delimiter,
        "zzzz".to_owned(),
        String::new(),
    ] {
        let got = verdict(body.as_bytes(), &owned());
        assert!(
            matches!(got, Err(Refused::Unreadable(_))),
            "{body:?}: {got:?}"
        );
    }
}

#[test]
fn an_endless_command_list_is_refused_unread() {
    let line = pkt(&format!("{A} {B} refs/heads/moor/12-fix\n"));
    let body = line.repeat(20_000);
    let got = verdict(body.as_bytes(), &owned());
    assert!(matches!(got, Err(Refused::Unreadable(_))), "{got:?}");
}

/// Every data line of `bytes`, through the first flush.
fn lines(mut bytes: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    while let Ok(decode::Stream::Complete {
        line,
        bytes_consumed,
    }) = decode::streaming(bytes)
    {
        match line {
            PacketLineRef::Data(d) => out.push(d.to_vec()),
            _ => break,
        }
        bytes = bytes.get(bytes_consumed..).unwrap_or_default();
    }
    out
}

#[test]
fn the_report_refuses_every_update_as_git_shows_a_hook_refusal() {
    let body = push(
        &[(A, B, "refs/heads/moor/12-fix"), (A, B, "refs/heads/main")],
        "report-status",
    );
    let commands = read_commands(&mut &body[..]).unwrap();
    let out = report(&commands, "moor refuses it").unwrap();
    let got: Vec<String> = lines(&out)
        .iter()
        .map(|l| String::from_utf8_lossy(l).into_owned())
        .collect();
    assert_eq!(
        got,
        [
            "unpack ok\n",
            "ng refs/heads/moor/12-fix moor refuses it\n",
            "ng refs/heads/main moor refuses it\n",
        ]
    );
    assert!(out.ends_with(b"0000"));
}

#[test]
fn on_a_side_band_the_report_rides_band_one_and_the_message_band_two() {
    let body = push(&[(A, B, "refs/heads/main")], "report-status side-band-64k");
    let commands = read_commands(&mut &body[..]).unwrap();
    let out = report(&commands, "moor refuses it").unwrap();
    let bands = lines(&out);
    assert_eq!(bands.first().unwrap(), b"\x02moor refuses it\n");
    let (band, data) = bands.get(1).unwrap().split_first().unwrap();
    assert_eq!(*band, 1);
    let inner = lines(data);
    assert_eq!(
        inner.get(1).unwrap(),
        b"ng refs/heads/main moor refuses it\n"
    );
}

#[test]
fn with_no_status_report_asked_there_is_no_report() {
    let body = push(&[(A, B, "refs/heads/main")], "side-band-64k");
    let commands = read_commands(&mut &body[..]).unwrap();
    assert_eq!(report(&commands, "moor refuses it"), None);
}
