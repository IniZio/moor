//! Escaping for text Slack would otherwise read as markup.

/// Escapes the characters Slack reads as markup, so `<!channel>`, mentions and links show as typed.
pub(crate) fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Escapes `text` for a code span: markup as [`escape`] does, and each backtick, which would
/// end the span early, as the look-alike `ˋ`.
pub(crate) fn code(text: &str) -> String {
    escape(text).replace('`', "ˋ")
}

#[cfg(test)]
mod tests;
