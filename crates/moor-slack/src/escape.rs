//! Escaping for text Slack would otherwise read as markup.

/// Escapes the characters Slack reads as markup, so `<!channel>`, mentions and links show as typed.
pub(crate) fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// `text` escaped as [`escape`] does, in a code span unless it holds a backtick. Slack has no
/// escape for a backtick inside a span, so such text is posted plain and a copy gives it exactly.
pub(crate) fn code(text: &str) -> String {
    let text = escape(text);
    if text.contains('`') {
        text
    } else {
        format!("`{text}`")
    }
}

#[cfg(test)]
mod tests;
