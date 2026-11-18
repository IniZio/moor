//! The short title a Box goes by, so every front shows the same one.

/// The most characters a title holds, the ellipsis included.
pub const TITLE_MAX: usize = 72;

/// The shortest sentence worth cutting at; a shorter lead such as "Hi." keeps going.
const SENTENCE_MIN: usize = 12;

/// A title for a Box asked `prompt`: its first line, cut at the first sentence end, cut again to
/// [`TITLE_MAX`] characters at a word boundary. A blank or missing prompt gives `name`.
#[must_use]
pub fn title_of(prompt: Option<&str>, name: &str) -> String {
    let line = prompt
        .and_then(|p| p.lines().map(clean).find(|l| !l.is_empty()))
        .unwrap_or_default();
    if line.is_empty() {
        return name.to_owned();
    }
    clip(first_sentence(&line))
}

/// `line` without the list, quote or heading marks a prompt may open with.
fn clean(line: &str) -> String {
    line.trim()
        .trim_start_matches(|c: char| matches!(c, '#' | '>' | '-' | '*' | '`') || c.is_whitespace())
        .trim()
        .to_owned()
}

/// `line` up to and including its first sentence end: a . ! or ? followed by a space.
fn first_sentence(line: &str) -> &str {
    let mut seen = 0;
    let mut chars = line.char_indices().peekable();
    while let Some((at, c)) = chars.next() {
        seen += 1;
        if matches!(c, '.' | '!' | '?')
            && seen >= SENTENCE_MIN
            && chars.peek().is_some_and(|(_, next)| next.is_whitespace())
        {
            return line.get(..at + c.len_utf8()).unwrap_or(line);
        }
    }
    line
}

/// `text` cut to [`TITLE_MAX`] characters, at a word boundary when there is one, with an ellipsis.
fn clip(text: &str) -> String {
    let text = text.trim_end_matches(['.', ':']).trim_end();
    if text.chars().count() <= TITLE_MAX {
        return text.to_owned();
    }
    let room: String = text.chars().take(TITLE_MAX - 1).collect();
    let cut = match room.rfind(char::is_whitespace) {
        Some(at) if at >= TITLE_MAX / 2 => room.get(..at).unwrap_or(&room),
        _ => &room,
    };
    format!("{}…", cut.trim_end_matches([',', ';', ':', '-', ' ']))
}
