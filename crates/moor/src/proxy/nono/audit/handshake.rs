//! Names why an intercepted TLS handshake ended, from nono's denial reason.
//!
//! Only a fixed set of values leaves this module. The reason text from nono is
//! matched, never copied, except for a TLS alert name, which is kept only when
//! it is plain letters and digits.

/// Prefix of the text rustls gives for an alert the client sent.
const ALERT: &str = "received fatal alert: ";

/// The value for a client that hung up without sending an alert.
pub(super) const CLIENT_CLOSED: &str = "client_closed";

/// The value recorded for a handshake that ended with `reason`.
///
/// One of `client_closed`, `unknown_ca_alert`, `other_alert:<name>` or `other`.
pub(super) fn reason(reason: &str) -> String {
    if let Some(at) = reason.find(ALERT) {
        let name: String = reason
            .get(at + ALERT.len()..)
            .unwrap_or_default()
            .chars()
            .take_while(char::is_ascii_alphanumeric)
            .take(32)
            .collect();
        return match name.as_str() {
            "" => "other".to_owned(),
            "UnknownCA" => "unknown_ca_alert".to_owned(),
            _ => format!("other_alert:{name}"),
        };
    }
    let text = reason.to_ascii_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|n| text.contains(n));
    if has(&[
        "handshake eof",
        "unexpected eof",
        "connection reset",
        "broken pipe",
        "close_notify",
    ]) {
        CLIENT_CLOSED.to_owned()
    } else {
        "other".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::reason;

    #[test]
    fn nono_reasons_map_to_the_closed_set() {
        let cases = [
            ("tls handshake failed: tls handshake eof", "client_closed"),
            (
                "tls handshake failed: Connection reset by peer (os error 104)",
                "client_closed",
            ),
            (
                "tls handshake failed: received fatal alert: UnknownCA",
                "unknown_ca_alert",
            ),
            (
                "tls handshake failed: received fatal alert: BadCertificate",
                "other_alert:BadCertificate",
            ),
            ("tls handshake failed: received fatal alert: ", "other"),
            (
                "tls handshake failed: Connection timed out (os error 110)",
                "other",
            ),
            ("tls handshake failed: something new", "other"),
            ("", "other"),
        ];
        for (text, want) in cases {
            assert_eq!(reason(text), want, "{text}");
        }
    }

    #[test]
    fn an_alert_name_is_capped_at_32_characters() {
        let long = "A".repeat(40);
        assert_eq!(
            reason(&format!("received fatal alert: {long}")),
            format!("other_alert:{}", "A".repeat(32))
        );
    }

    #[test]
    fn an_alert_name_is_cut_at_the_first_non_alphanumeric() {
        assert_eq!(
            reason("received fatal alert: Odd Name\nsecret=hunter2"),
            "other_alert:Odd"
        );
    }
}
