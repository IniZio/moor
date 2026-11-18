//! The plain text of a line of markdown, for the one-line summary.

/// Whether `line` only draws: a fence, a rule or a table's divider.
fn draws(line: &str) -> bool {
    let t = line.trim();
    t.starts_with("```")
        || t.starts_with("~~~")
        || (t.len() >= 3 && t.chars().all(|c| "-*_=| :".contains(c)))
}

/// The first line of `s` that says something, as plain text.
#[must_use]
pub(super) fn first_plain_line(s: &str) -> String {
    s.lines()
        .find(|l| !l.trim().is_empty() && !draws(l))
        .map(plain)
        .unwrap_or_default()
}

/// `line` without its markdown: heading and quote marks, list bullets, emphasis, code ticks, and
/// links and images as their text.
fn plain(line: &str) -> String {
    let mut t = line.trim();
    t = t.trim_start_matches('>').trim_start();
    t = t.trim_start_matches('#').trim_start();
    for bullet in ["- [ ] ", "- [x] ", "- ", "* ", "+ "] {
        if let Some(rest) = t.strip_prefix(bullet) {
            t = rest.trim_start();
            break;
        }
    }
    let digits = t.chars().take_while(char::is_ascii_digit).count();
    if (1..=3).contains(&digits) {
        if let Some(rest) = t.get(digits..).and_then(|r| r.strip_prefix(". ")) {
            t = rest.trim_start();
        }
    }
    let chars: Vec<char> = t.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while let Some(&c) = chars.get(i) {
        match c {
            '`' | '*' | '~' => {}
            '_' if chars.get(i + 1) == Some(&'_') => {}
            '!' if chars.get(i + 1) == Some(&'[') => {}
            '[' => {
                if let Some((text, next)) = link(&chars, i) {
                    out.push_str(&plain(&text));
                    i = next;
                    continue;
                }
                out.push(c);
            }
            _ => out.push(c),
        }
        i += 1;
    }
    out.trim().to_owned()
}

/// The text of the link that opens at `at` (a `[`), and where the link ends.
fn link(chars: &[char], at: usize) -> Option<(String, usize)> {
    let close = chars.get(at..)?.iter().position(|c| *c == ']')? + at;
    if chars.get(close + 1) != Some(&'(') {
        return None;
    }
    let end = chars.get(close + 2..)?.iter().position(|c| *c == ')')? + close + 2;
    Some((chars.get(at + 1..close)?.iter().collect(), end + 1))
}
