//! Every `yaml` block in `docs/config.md` parses with the real config parsers.

use super::{parse_controller, parse_repo, parse_user};

const DOC: &str = include_str!("../../../../docs/config.md");

#[derive(Clone, Copy, Debug)]
enum Kind {
    User,
    Repo,
    Controller,
    Profile,
}

struct Block {
    line: usize,
    kind: Option<Kind>,
    text: String,
}

fn kind_of(heading: &str) -> Option<Kind> {
    match heading {
        "User config" => Some(Kind::User),
        "`.moor.yaml`" => Some(Kind::Repo),
        "Controller config" => Some(Kind::Controller),
        _ => None,
    }
}

/// Each yaml fence, classified by the nearest preceding `## ` heading, except that the
/// per-user agent profile's `### ` section holds `moor-profile.yaml` examples, which the
/// profile's own tests parse.
fn yaml_blocks() -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut section = None;
    let mut kind = None;
    let mut open: Option<Block> = None;
    for (i, line) in DOC.lines().enumerate() {
        if let Some(block) = open.as_mut() {
            if line.trim_end() == "```" {
                blocks.extend(open.take());
            } else {
                block.text.push_str(line);
                block.text.push('\n');
            }
        } else if line.trim_end() == "```yaml" {
            open = Some(Block {
                line: i + 1,
                kind,
                text: String::new(),
            });
        } else if let Some(heading) = line.strip_prefix("## ") {
            section = kind_of(heading.trim());
            kind = section;
        } else if let Some(heading) = line.strip_prefix("### ") {
            kind = if heading.trim() == "Per-user agent profile" {
                Some(Kind::Profile)
            } else {
                section
            };
        }
    }
    assert!(open.is_none(), "docs/config.md ends inside a yaml fence");
    blocks
}

#[test]
fn every_yaml_example_in_config_md_parses() {
    let blocks = yaml_blocks();
    assert!(blocks.len() >= 4, "found only {} yaml blocks", blocks.len());
    let failures: Vec<String> = blocks
        .iter()
        .filter_map(|b| {
            let file = "docs/config.md";
            let err = match b.kind {
                None => Some("yaml block under no known `##` section".to_string()),
                Some(Kind::User) => parse_user(file, &b.text).err().map(|e| e.to_string()),
                Some(Kind::Repo) => parse_repo(file, &b.text).err().map(|e| e.to_string()),
                Some(Kind::Controller) => {
                    parse_controller(file, &b.text).err().map(|e| e.to_string())
                }
                Some(Kind::Profile) => None,
            };
            err.map(|e| format!("docs/config.md:{} ({:?}): {e}", b.line, b.kind))
        })
        .collect();
    assert!(
        failures.is_empty(),
        "doc examples rejected by the parser:\n{}",
        failures.join("\n")
    );
}
