//! Where a quote sits in the source: an exact match first, then a match that folds white space
//! runs, Unicode compatibility forms and curly quotes. moor never guesses between places.

use unicode_normalization::char::decompose_compatible;

use super::{Quote, Span};

/// Why a quote has no one place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Miss {
    /// It has no words.
    Empty,
    /// It is nowhere.
    NotFound,
    /// It is in this many places.
    Ambiguous(usize),
}

/// The one place of `quote` in `source`.
pub(super) fn place(source: &str, quote: &Quote) -> Result<Span, Miss> {
    if quote.exact.trim().is_empty() {
        return Err(Miss::Empty);
    }
    match exact(source, quote).as_slice() {
        [one] => return Ok(*one),
        [] => {}
        many => return Err(Miss::Ambiguous(many.len())),
    }
    match folded(source, quote).as_slice() {
        [one] => Ok(*one),
        [] => Err(Miss::NotFound),
        many => Err(Miss::Ambiguous(many.len())),
    }
}

/// Every place the quote's words, prefix and suffix match byte for byte.
fn exact(source: &str, q: &Quote) -> Vec<Span> {
    source
        .char_indices()
        .filter_map(|(start, _)| {
            let end = start + q.exact.len();
            let hit = source.get(start..end) == Some(q.exact.as_str())
                && source
                    .get(..start)
                    .is_some_and(|b| ends(b, q.prefix.as_deref()))
                && source
                    .get(end..)
                    .is_some_and(|a| starts(a, q.suffix.as_deref()));
            hit.then_some(Span::new(start, end))
        })
        .collect()
}

/// Whether `text` ends with `prefix`, when there is one.
fn ends(text: &str, prefix: Option<&str>) -> bool {
    prefix.is_none_or(|p| text.ends_with(p))
}

/// Whether `text` starts with `suffix`, when there is one.
fn starts(text: &str, suffix: Option<&str>) -> bool {
    suffix.is_none_or(|s| text.starts_with(s))
}

/// One folded char and the source bytes it came from.
#[derive(Debug, Clone, Copy)]
struct Folded {
    /// The char.
    c: char,
    /// The source bytes: one char, or a whole white space run.
    unit: (usize, usize),
    /// Whether it stands for white space.
    space: bool,
}

/// `text` folded, each char tied to the source bytes it came from.
fn fold(text: &str) -> Vec<Folded> {
    let mut out: Vec<Folded> = Vec::with_capacity(text.len());
    for (at, c) in text.char_indices() {
        let end = at + c.len_utf8();
        if c.is_whitespace() {
            match out.last_mut() {
                Some(last) if last.space => last.unit.1 = end,
                _ => out.push(Folded {
                    c: ' ',
                    unit: (at, end),
                    space: true,
                }),
            }
            continue;
        }
        let unit = (at, end);
        match straight(c) {
            Some(c) => out.push(Folded {
                c,
                unit,
                space: false,
            }),
            None => decompose_compatible(c, |c| {
                out.push(Folded {
                    c,
                    unit,
                    space: false,
                });
            }),
        }
    }
    out
}

/// The straight quote a curly one stands for.
const fn straight(c: char) -> Option<char> {
    match c {
        '\u{2018}' | '\u{2019}' | '\u{201a}' | '\u{201b}' => Some('\''),
        '\u{201c}' | '\u{201d}' | '\u{201e}' | '\u{201f}' => Some('"'),
        _ => None,
    }
}

/// The folded chars of `text`.
fn chars(text: &str) -> Vec<char> {
    fold(text).into_iter().map(|f| f.c).collect()
}

/// Every place the folded words match, starting and ending on whole source chars.
fn folded(source: &str, q: &Quote) -> Vec<Span> {
    let src = fold(source);
    let all: Vec<char> = src.iter().map(|f| f.c).collect();
    let words = chars(q.exact.trim());
    let prefix = q.prefix.as_deref().map(chars);
    let suffix = q.suffix.as_deref().map(chars);
    (0..all.len())
        .filter_map(|i| {
            let j = i + words.len();
            let (first, last) = (src.get(i)?, src.get(j.checked_sub(1)?)?);
            let whole = (i == 0 || src.get(i - 1).is_some_and(|f| f.unit != first.unit))
                && src.get(j).is_none_or(|f| f.unit != last.unit);
            let hit = all.get(i..j)? == words.as_slice()
                && whole
                && prefix
                    .as_ref()
                    .is_none_or(|p| all.get(..i).is_some_and(|b| b.ends_with(p)))
                && suffix
                    .as_ref()
                    .is_none_or(|s| all.get(j..).is_some_and(|a| a.starts_with(s)));
            hit.then_some(Span::new(first.unit.0, last.unit.1))
        })
        .collect()
}
