//! Characters an author name must not carry.

/// Unicode format characters (category Cf) and the line and paragraph separators (Zl, Zp), as
/// of Unicode 15.1: bidi overrides, zero-width marks and the BOM can make an author name read as
/// other text.
const FORMAT_CHARS: &[(char, char)] = &[
    ('\u{ad}', '\u{ad}'),
    ('\u{600}', '\u{605}'),
    ('\u{61c}', '\u{61c}'),
    ('\u{6dd}', '\u{6dd}'),
    ('\u{70f}', '\u{70f}'),
    ('\u{890}', '\u{891}'),
    ('\u{8e2}', '\u{8e2}'),
    ('\u{180e}', '\u{180e}'),
    ('\u{200b}', '\u{200f}'),
    ('\u{2028}', '\u{202e}'),
    ('\u{2060}', '\u{2064}'),
    ('\u{2066}', '\u{206f}'),
    ('\u{feff}', '\u{feff}'),
    ('\u{fff9}', '\u{fffb}'),
    ('\u{110bd}', '\u{110bd}'),
    ('\u{110cd}', '\u{110cd}'),
    ('\u{13430}', '\u{1343f}'),
    ('\u{1bca0}', '\u{1bca3}'),
    ('\u{1d173}', '\u{1d17a}'),
    ('\u{e0001}', '\u{e0001}'),
    ('\u{e0020}', '\u{e007f}'),
];

/// Whether `c` is a control, format or separator character a profile name must not carry.
pub(super) fn spoofs(c: char) -> bool {
    c.is_control() || FORMAT_CHARS.iter().any(|&(lo, hi)| (lo..=hi).contains(&c))
}
