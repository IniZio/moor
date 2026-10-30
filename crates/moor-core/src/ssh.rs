//! ssh words that come from crabbox: reading the line it prints and checking what they pin.

pub mod pin;

/// Splits one line of single-quoted shell words; `None` when a quote is left open.
#[must_use]
pub fn split_words(line: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut started = false;
    let mut quoted = false;
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match (quoted, c) {
            (true, '\'') => quoted = false,
            (true, _) => cur.push(c),
            (false, '\'') => {
                quoted = true;
                started = true;
            }
            (false, '\\') => {
                cur.push(chars.next()?);
                started = true;
            }
            (false, c) if c.is_whitespace() => {
                if started {
                    words.push(std::mem::take(&mut cur));
                    started = false;
                }
            }
            (false, _) => {
                cur.push(c);
                started = true;
            }
        }
    }
    if quoted {
        return None;
    }
    if started {
        words.push(cur);
    }
    Some(words)
}

#[cfg(test)]
mod tests;
