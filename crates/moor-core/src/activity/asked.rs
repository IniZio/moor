//! A question the agent's last reply puts to the person.

/// What follows a question when the agent recommends an answer to it.
const RECOMMEND: &str = "? Recommend:";

/// The question `said`, the agent's reply at the end of its turn, puts to the person; none when
/// it asks nothing. Lines inside code fences are not read.
///
/// A reply asks when a line puts a question with a recommended answer, `... ? Recommend: ...`,
/// or when its last line ends in a question mark. The first such question is the one named;
/// more are counted after it.
#[must_use]
pub fn asked(said: &str) -> Option<String> {
    let mut fenced = false;
    let mut recommended = Vec::new();
    let mut last = None;
    for raw in said.lines() {
        let line = raw.trim();
        if line.starts_with("```") {
            fenced = !fenced;
            continue;
        }
        if fenced || line.is_empty() {
            continue;
        }
        let line = plain(line);
        if let Some((question, _)) = line.split_once(RECOMMEND) {
            recommended.push(format!("{}?", last_sentence(question)));
        }
        last = Some(line);
    }
    let Some(first) = recommended.first() else {
        return last.filter(|l| l.ends_with('?'));
    };
    Some(match recommended.len() - 1 {
        0 => first.clone(),
        more => format!("{first} (and {more} more)"),
    })
}

/// The last sentence of `text`: a question may follow what led up to it on its line.
fn last_sentence(text: &str) -> &str {
    let after_stop = |stop: &str| text.rsplit_once(stop).map(|(_, rest)| rest);
    [after_stop(". "), after_stop("! ")]
        .into_iter()
        .flatten()
        .min_by_key(|rest| rest.len())
        .unwrap_or(text)
        .trim()
}

/// `line` without its emphasis, its list marker or its `#N` number.
fn plain(line: &str) -> String {
    let line = line.replace("**", "");
    let line = line.trim_matches(|c: char| c == '_' || c == '*' || c.is_whitespace());
    let line = line
        .strip_prefix("- ")
        .or_else(|| line.strip_prefix("* "))
        .unwrap_or(line);
    let unnumbered = line.trim_start_matches(|c: char| c.is_ascii_digit());
    let line = (unnumbered.len() < line.len())
        .then(|| {
            unnumbered
                .strip_prefix(". ")
                .or_else(|| unnumbered.strip_prefix(") "))
        })
        .flatten()
        .unwrap_or(line);
    let line = line.strip_prefix('#').map_or(line, |rest| {
        rest.trim_start_matches(|c: char| c.is_ascii_digit())
            .trim_start()
    });
    line.trim().to_owned()
}

#[cfg(test)]
mod tests;
