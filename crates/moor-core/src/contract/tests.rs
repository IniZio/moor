//! Tests for the contract file, its lock, its budget and the launch text.

use super::*;

/// The contract, which the built-in file must parse to.
fn built_in() -> &'static Contract {
    contract().unwrap()
}

/// The longest skill path a launch writes: a credential directory under the Box home.
const LONG_PATH: &str = "/home/agent/.moor-cred-moor-some-long-box-name-0123456789abcdef/skills";

/// Every skill of `situations`, at its longest path.
fn entries(situations: &[Situation]) -> Vec<SkillEntry> {
    skills()
        .unwrap()
        .iter()
        .filter(|s| s.situations.iter().any(|x| situations.contains(x)))
        .map(|s| SkillEntry {
            name: s.name,
            description: s.description,
            path: format!("{LONG_PATH}/{}/SKILL.md", s.name),
        })
        .collect()
}

fn render(situations: &[Situation], moor_tools: bool) -> String {
    Brief {
        contract: built_in(),
        situations,
        moor_tools,
        skills: &entries(situations),
    }
    .render()
}

#[test]
fn the_built_in_file_has_a_version_and_the_every_box_run_and_place_sections() {
    let c = built_in();
    assert!(c.version >= 1);
    let order: Vec<Situation> = c.sections.iter().map(|(s, _)| *s).collect();
    assert_eq!(
        order,
        [
            Situation::Every,
            Situation::Box,
            Situation::Run,
            Situation::Place
        ]
    );
}

#[test]
fn the_lock_pins_the_version_to_the_file() {
    let field = |name: &str| {
        LOCK.lines()
            .find_map(|l| l.strip_prefix(name)?.strip_prefix(" = "))
            .unwrap()
            .to_owned()
    };
    let (version, sha) = (field("version"), field("sha256"));
    assert_eq!(
        (version.as_str(), sha.as_str()),
        (
            built_in().version.to_string().as_str(),
            sha256(SOURCE).as_str()
        ),
        "agents/distro/contract.md changed: raise its Version line and write the new version and \
         sha256 into agents/distro/contract.lock"
    );
}

#[test]
fn each_section_and_statement_keeps_to_the_budget() {
    for (situation, statements) in &built_in().sections {
        let cap = if *situation == Situation::Every {
            10
        } else {
            6
        };
        assert!(
            statements.len() <= cap,
            "{} has {}",
            situation.key(),
            statements.len()
        );
        for s in statements {
            assert!(
                s.text.chars().count() <= 240,
                "[{}] is {} chars",
                s.key,
                s.text.len()
            );
        }
    }
}

#[test]
fn every_launch_with_its_generated_blocks_fits_4096_characters() {
    let launches: [&[Situation]; 3] = [
        &[Situation::Every, Situation::Box],
        &[Situation::Every, Situation::Box, Situation::Run],
        &[Situation::Every, Situation::Place],
    ];
    for situations in launches {
        let text = render(situations, true);
        assert!(
            text.chars().count() <= 4096,
            "{} chars:\n{text}",
            text.len()
        );
    }
}

#[test]
fn a_launch_reads_its_sections_in_order_then_tools_then_skills() {
    let text = render(&[Situation::Every, Situation::Box, Situation::Run], true);
    let at = |s: &str| text.find(s).unwrap();
    assert!(
        text.starts_with("# moor agent contract\n\nVersion "),
        "{text}"
    );
    assert!(at("## every") < at("## box") && at("## box") < at("## run"));
    assert!(at("## run") < at("## moor tools") && at("## moor tools") < at("## moor skills"));
    assert!(text.contains("On the MCP server named moor: report_output, report_step."));
    assert!(text.contains("- moor-attach-files: "));
    assert!(text.contains(&format!("`{LONG_PATH}/moor-attach-files/SKILL.md`")));
}

#[test]
fn a_task_box_reads_no_run_lines_and_no_run_tool() {
    let text = render(&[Situation::Every, Situation::Box], true);
    assert!(
        !text.contains("## run") && !text.contains("[gates]"),
        "{text}"
    );
    assert!(
        text.contains("On the MCP server named moor: report_output."),
        "{text}"
    );
}

#[test]
fn a_place_reads_its_own_lines_and_tools_and_no_box_or_run_lines() {
    let text = render(&[Situation::Every, Situation::Place], true);
    assert!(text.contains("## place\n\n- [repos]"), "{text}");
    assert!(
        !text.contains("## box") && !text.contains("## run") && !text.contains("[branch]"),
        "{text}"
    );
    assert!(
        text.contains(
            "On the MCP server named moor: report_output, start_box, exec_in_box, exec_wait, \
             close_box."
        ),
        "{text}"
    );
}

#[test]
fn a_place_starts_a_box_without_setup_only_to_find_why_setup_failed() {
    let text = render(&[Situation::Every, Situation::Place], true);
    assert!(
        text.contains(
            "- [no-setup] Start a work Box with no_setup only to find out why a setup line failed."
        ),
        "{text}"
    );
}

#[test]
fn a_place_sends_repo_work_to_a_work_box() {
    let text = render(&[Situation::Every, Situation::Place], true);
    assert!(
        text.contains(
            "- [answer] This place is for reading and answering. Anything that changes a repo, \
             runs its code or its tools, or installs a tool for that work happens in a work Box \
             you start with start_box. moor refuses repo writes from here."
        ),
        "{text}"
    );
}

#[test]
fn without_the_moor_server_no_tool_block() {
    let text = render(&[Situation::Every, Situation::Box], false);
    assert!(!text.contains("## moor tools"), "{text}");
}

#[test]
fn each_built_in_skill_is_named_moor_and_described() {
    let all = skills().unwrap();
    assert!(all.iter().any(|s| s.name == "moor-attach-files"));
    for s in all {
        assert!(
            s.name.starts_with("moor-") && !s.description.is_empty(),
            "{s:?}"
        );
    }
}

#[test]
fn a_malformed_file_is_refused_with_its_reason() {
    let ok = "# t\n\nVersion 2\n\nPreamble.\n\n## every\n\n- [a] text.\n";
    assert_eq!(parse::contract(ok).unwrap().version, 2);
    let cases = [
        ("# t\n\nPreamble.\n\n## every\n\n- [a] x\n", "Version"),
        (
            "# t\n\nVersion 1\n\nP.\n\n## reader\n\n- [a] x\n",
            "unknown section",
        ),
        (
            "# t\n\nVersion 1\n\nP.\n\n## every\n\n- [a] x\n- [a] y\n",
            "twice",
        ),
        (
            "# t\n\nVersion 1\n\nP.\n\n## every\n\nloose words\n",
            "not a statement",
        ),
        (
            "# t\n\nVersion 1\n\nP.\n\n## every\n\n- [A] x\n",
            "lower-case",
        ),
    ];
    for (src, why) in cases {
        let err = parse::contract(src).unwrap_err();
        assert!(err.contains(why), "{src:?}: {err}");
    }
}

#[test]
fn a_skill_outside_the_moor_prefix_or_without_a_version_is_refused() {
    let file = "---\nname: attach\ndescription: d\nmetadata:\n  version: \"1\"\n---\nbody\n";
    assert!(parse::skill(file, &[]).unwrap_err().contains("moor-"));
    let file = "---\nname: moor-x\ndescription: d\n---\nbody\n";
    assert!(parse::skill(file, &[]).unwrap_err().contains("version"));
}
