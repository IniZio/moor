//! Menu reading: the numbered choices an agent pane is waiting on.

/// The label of a menu line such as `❯ 1. Yes`, with its number.
fn menu_line(line: &str) -> Option<(u32, &str)> {
    let rest = line
        .trim_start()
        .trim_start_matches(['❯', '>', '›'])
        .trim_start();
    let (num, label) = rest.split_once(". ")?;
    if num.is_empty() || num.len() > 2 || !num.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let label = label.trim();
    if label.is_empty() {
        return None;
    }
    Some((num.parse().ok()?, label))
}

/// The choices of the last numbered menu in the pane text, in order.
///
/// A menu is a run of at least two consecutive numbered lines counting 1, 2, 3 and so
/// on; anything else gives no options.
#[must_use]
pub fn menu_options(tail: &str) -> Vec<String> {
    let mut block: Vec<(u32, &str)> = Vec::new();
    for line in tail.lines().rev().filter(|l| !l.trim().is_empty()) {
        match menu_line(line) {
            Some(item) => block.push(item),
            None if block.is_empty() => {}
            None => break,
        }
    }
    block.reverse();
    let counting = block.iter().zip(1u32..).all(|((n, _), want)| *n == want);
    if block.len() < 2 || !counting {
        return Vec::new();
    }
    block.into_iter().map(|(_, l)| l.to_owned()).collect()
}
