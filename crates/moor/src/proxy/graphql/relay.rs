//! Relays an upstream response to the client, checking it first when the request asks.

use std::io::{BufReader, Write};

use super::check::{checked, Relay};
use super::http::{copy_body, read_response_head, response_framing, wants_close, Framing, Head};
use super::serve::{bad, Client, Flow};
use super::upstream::TlsStream;
use super::Shared;

/// Relays the response, setting `sent` once a byte reached the client. A
/// rejection is noted before any byte is relayed.
pub(super) fn relay(
    shared: &Shared,
    conn: &mut BufReader<TlsStream>,
    client: &mut Client,
    head: &Head,
    sent: &mut bool,
    how: Relay<'_>,
) -> Flow {
    let resp = loop {
        let Ok(resp) = read_response_head(conn) else {
            return Flow::Close;
        };
        if (100..200).contains(&resp.status) && resp.status != 101 {
            if client.get_mut().write_all(&resp.raw).is_err() || client.get_mut().flush().is_err() {
                *sent = true;
                return Flow::Close;
            }
            *sent = true;
            continue;
        }
        break resp;
    };
    if matches!(how, Relay::Anonymous) && matches!(resp.status, 401 | 403) {
        shared.excuse(resp.status);
    }
    *sent = true;
    let Ok(framing) = response_framing(&resp, &head.method) else {
        return Flow::Close;
    };
    if let Relay::Checked(linked) = how {
        match checked(&resp, &framing, conn, linked, &shared.repos) {
            Ok((head, body))
                if client
                    .get_mut()
                    .write_all(&head)
                    .and_then(|()| client.get_mut().write_all(&body))
                    .is_ok() => {}
            Ok(_) => return Flow::Close,
            Err(msg) => return bad(client, 502, &msg),
        }
    } else {
        let out = client.get_mut();
        if out.write_all(&resp.raw).is_err() || copy_body(conn, out, &framing).is_err() {
            return Flow::Close;
        }
    }
    let out = client.get_mut();
    if out.flush().is_err()
        || resp.status == 101
        || framing == Framing::UntilClose
        || wants_close(&head.headers)
        || wants_close(&resp.headers)
    {
        return Flow::Close;
    }
    Flow::Continue
}
