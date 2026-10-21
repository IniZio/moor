//! Escaping for text Slack would otherwise read as markup.

/// Escapes the characters Slack reads as markup, so `<!channel>`, mentions and links show as typed.
pub(crate) fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests;
