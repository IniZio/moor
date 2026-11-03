//! Names why the Proxy could not reach an upstream, from nono's denial reason.
//!
//! Only a fixed set of values leaves this module. The reason text from nono
//! can carry addresses and server messages, so it is matched, never copied.

/// The value recorded for a connect failure whose cause is `reason`.
///
/// One of `refused`, `dns`, `timeout`, `reset`, `tls` or `other`.
pub(super) fn reason(reason: &str) -> &'static str {
    let text = reason.to_ascii_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|n| text.contains(n));
    // TLS goes first because its text often embeds the socket error beneath it.
    if has(&["tls", "certificate", "handshake"]) {
        "tls"
    } else if has(&["dns resolution", "no addresses", "failed to lookup"]) {
        "dns"
    } else if has(&["refused"]) {
        "refused"
    } else if has(&["timed out", "timeout"]) {
        "timeout"
    } else if has(&["reset", "broken pipe", "closed before", "unexpected eof"]) {
        "reset"
    } else {
        "other"
    }
}

#[cfg(test)]
mod tests {
    use super::reason;

    #[test]
    fn nono_reasons_map_to_the_closed_set() {
        let cases = [
            ("Connection refused (os error 111)", "refused"),
            ("DNS resolution returned no addresses", "dns"),
            ("no addresses to connect to", "dns"),
            ("connection timed out", "timeout"),
            ("Connection timed out (os error 110)", "timeout"),
            ("Connection reset by peer (os error 104)", "reset"),
            (
                "upstream closed before sending an upgrade response",
                "reset",
            ),
            (
                "TLS handshake failed: invalid peer certificate: UnknownIssuer",
                "tls",
            ),
            (
                "TLS handshake failed: Connection reset by peer (os error 104)",
                "tls",
            ),
            ("invalid server name for TLS", "tls"),
            ("pool request failed: something new", "other"),
            ("", "other"),
        ];
        for (text, want) in cases {
            assert_eq!(reason(text), want, "{text}");
        }
    }

    #[test]
    fn the_text_is_never_copied() {
        assert_eq!(reason("secret=hunter2 at 10.0.0.7:443"), "other");
    }
}
