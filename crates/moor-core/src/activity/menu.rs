//! Menu reading: the numbered choices an agent pane is waiting on.

/// The label of a menu line such as `❯ 1. Yes`, with its number.
#[must_use]
pub fn menu_line(line: &str) -> Option<(u32, &str)> {
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

/// The most lines of a prompt's detail.
const DETAIL_LINES: usize = 40;

/// Whether `text` only draws a rule or a box edge.
fn is_rule(text: &str) -> bool {
    !text.is_empty() && text.chars().all(|c| "─━═╭╮╰╯┌┐└┘-_╌┄┈ ".contains(c))
}

/// Whether `text` is a dashed rule, which Claude Code draws around the command it asks about.
fn is_fence(text: &str) -> bool {
    !text.is_empty() && text.chars().all(|c| "╌┄┈╍┅┉ ".contains(c))
}

/// Where the last numbered menu in `lines` starts.
fn menu_start(lines: &[&str]) -> Option<usize> {
    let mut start = None;
    for (i, line) in lines.iter().enumerate().rev() {
        if line.trim().is_empty() {
            continue;
        }
        if menu_line(line).is_some() {
            start = Some(i);
        } else if start.is_some() {
            break;
        }
    }
    start
}

/// What a permission prompt shows above its menu.
///
/// That is the command or file it asks to use. It sits between the last two dashed rules when the
/// prompt draws them; otherwise everything above the menu down to the rule that opens the prompt.
/// None when the pane shows no menu or nothing above it.
#[must_use]
pub fn menu_detail(tail: &str) -> Option<String> {
    let lines: Vec<&str> = tail.lines().collect();
    let start = menu_start(&lines)?;
    let mut above: Vec<&str> = Vec::new();
    for line in lines.get(..start)?.iter().rev() {
        let text = line.trim().trim_matches(['│', '┃', '|']).trim();
        if (is_rule(text) && !is_fence(text)) || above.len() >= DETAIL_LINES {
            break;
        }
        above.push(text);
    }
    above.reverse();
    let fences: Vec<usize> = (0..above.len())
        .filter(|i| above.get(*i).is_some_and(|t| is_fence(t)))
        .collect();
    let body = match fences.as_slice() {
        [.., open, close] => above.get(open + 1..*close)?,
        _ => &above[..],
    };
    let shown = body.join("\n");
    let shown = shown.trim();
    (!shown.is_empty()).then(|| shown.to_owned())
}
