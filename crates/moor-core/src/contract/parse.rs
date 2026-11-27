//! Parses the distro's contract file and a skill's front matter.

use super::{Contract, Situation, Skill, Statement};

/// The contract in `src`: a `# ` title, a `Version N` line, the preamble, then one `## `
/// section per situation, each a list of `- [key] text` statements.
pub(super) fn contract(src: &'static str) -> Result<Contract, String> {
    let (head, body) = src
        .split_once("\n## ")
        .ok_or("the contract has no section")?;
    let (title_line, after) = head.split_once('\n').unwrap_or((head, ""));
    let title = title_line
        .strip_prefix("# ")
        .ok_or("the contract does not start with a # title")?;
    let after = after.trim_start();
    let (version_line, preamble) = after.split_once('\n').unwrap_or((after, ""));
    let version = version_line
        .strip_prefix("Version ")
        .and_then(|n| n.trim().parse().ok())
        .ok_or("no Version N line under the title")?;
    let preamble = preamble.trim();
    if preamble.is_empty() || preamble.contains("\n#") {
        return Err("the preamble must be plain paragraphs before the first section".into());
    }
    let mut sections: Vec<(Situation, Vec<Statement>)> = Vec::new();
    for chunk in body.split("\n## ") {
        let (key, rest) = chunk.split_once('\n').unwrap_or((chunk, ""));
        let situation =
            Situation::of(key.trim()).ok_or_else(|| format!("unknown section `## {key}`"))?;
        if sections.iter().any(|(s, _)| *s == situation) {
            return Err(format!("section `## {key}` appears twice"));
        }
        sections.push((situation, statements(key, rest)?));
    }
    let mut keys: Vec<&str> = sections
        .iter()
        .flat_map(|(_, st)| st)
        .map(|s| s.key)
        .collect();
    keys.sort_unstable();
    if let Some(w) = keys.windows(2).find(|w| w.first() == w.get(1)) {
        return Err(format!("key [{}] is used twice", w.first().unwrap_or(&"")));
    }
    Ok(Contract {
        version,
        title,
        preamble,
        sections,
    })
}

/// The statements of section `key` in `rest`; blank lines between them are allowed.
fn statements(key: &str, rest: &'static str) -> Result<Vec<Statement>, String> {
    let mut out = Vec::new();
    for line in rest.lines().filter(|l| !l.trim().is_empty()) {
        let (k, text) = line
            .strip_prefix("- [")
            .and_then(|l| l.split_once("] "))
            .ok_or_else(|| format!("`## {key}` holds a line that is not a statement: {line}"))?;
        let well_formed = k.starts_with(|c: char| c.is_ascii_lowercase())
            && k.chars().all(|c| c.is_ascii_lowercase() || c == '-');
        if !well_formed || text.trim().is_empty() {
            return Err(format!(
                "statement [{k}] needs a lower-case hyphenated key and text"
            ));
        }
        out.push(Statement { key: k, text });
    }
    if out.is_empty() {
        return Err(format!("`## {key}` has no statement"));
    }
    Ok(out)
}

/// The skill in `file`, from its front matter: `name`, `description` and `metadata.version`.
pub(super) fn skill(file: &'static str, situations: &'static [Situation]) -> Result<Skill, String> {
    let front = file
        .strip_prefix("---\n")
        .and_then(|r| r.split_once("\n---\n"))
        .map(|(f, _)| f)
        .ok_or("a skill file starts with front matter between --- lines")?;
    let field = |name: &str| {
        front
            .lines()
            .find_map(|l| l.strip_prefix(name)?.strip_prefix(": "))
            .map(str::trim)
            .filter(|v| !v.is_empty())
    };
    let name = field("name").ok_or("a skill needs a name")?;
    if !name.starts_with("moor-") {
        return Err(format!("built-in skill {name} must start with moor-"));
    }
    let description = field("description").ok_or_else(|| format!("{name} needs a description"))?;
    field("  version").ok_or_else(|| format!("{name} needs metadata.version"))?;
    Ok(Skill {
        name,
        description,
        file,
        situations,
    })
}
