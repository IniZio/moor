//! Tests for the brief.

use super::*;
use moor_core::contract::SkillEntry;
use moor_core::seam::engine::RunLaunch;

fn input(issue: Option<&str>) -> BriefInput<'_> {
    BriefInput {
        name: "t1",
        issue,
        branch: "moor/t1",
        setup_exit: None,
        repo_notes: None,
        shape: None,
        repos: &[],
        place: false,
        contract: moor_core::contract::contract().unwrap(),
        run: None,
        moor_tools: true,
        skills: &[],
    }
}

#[test]
fn a_place_brief_names_no_checkout_or_branch() {
    let out = render(&BriefInput {
        place: true,
        ..input(None)
    });
    assert!(out.contains("`Repos:` line"), "{out}");
    assert!(
        !out.contains("moor/t1") && !out.contains("The checkout is on branch"),
        "{out}"
    );
}

#[test]
fn the_clone_a_place_brief_names_passes_the_places_git_rule() {
    use moor_core::config::proxy::{resolve_rules, ProxyConfig};
    use nono_proxy::config::{CompiledEndpointRules, EndpointRule};

    let out = render(&BriefInput {
        place: true,
        ..input(None)
    });
    let url = out
        .split_whitespace()
        .find(|w| w.starts_with("https://github.com/OWNER/NAME"))
        .expect("the place brief names a clone URL")
        .trim_end_matches('`')
        .replace("OWNER/NAME", "o/n");
    let refs = format!("{}/info/refs", url.trim_start_matches("https://github.com"));
    let rules = resolve_rules(
        &ProxyConfig::default(),
        &[moor_core::seam::forge::ForgeRepo::new("o", "n")],
        false,
    )
    .unwrap();
    let git = rules.iter().find(|r| r.name == "github-git").unwrap();
    let allow: Vec<EndpointRule> = git
        .endpoint_rules
        .iter()
        .map(|r| EndpointRule {
            method: r.method.clone(),
            path: r.path.clone(),
        })
        .collect();
    let allow = CompiledEndpointRules::compile(&allow).unwrap();
    assert!(allow.is_allowed("GET", &refs), "{refs}");
}

#[test]
fn a_place_brief_names_start_box_close_box_and_the_artifacts_dir() {
    let out = render(&BriefInput {
        place: true,
        ..input(None)
    });
    let place = out.split("Reports are how").next().unwrap_or_default();
    for word in ["start_box", "exec_in_box", ".moor/artifacts/", "close_box"] {
        assert!(place.contains(word), "{word} in {place}");
    }
    assert!(!render(&input(None)).contains("start_box"));
}

#[test]
fn a_place_brief_points_files_at_the_work_box_and_claims_no_401_for_other_repos() {
    let out = render(&BriefInput {
        place: true,
        ..input(None)
    });
    assert!(
        out.contains("in the work Box's `.moor/artifacts/`"),
        "{out}"
    );
    assert!(
        !out.contains("copy it into `.moor/artifacts/` in your working directory"),
        "{out}"
    );
    assert!(!out.contains("answers 401 or 404"), "{out}");
}

#[test]
fn a_place_brief_says_a_later_box_continues_the_task_s_branch_and_pull_request() {
    let out = render(&BriefInput {
        place: true,
        ..input(None)
    });
    let place = out.split("Reports are how").next().unwrap_or_default();
    for words in [
        "A later work Box of this task starts on the task's branch",
        "adds to the same pull request",
        "One work Box of a task is open at a time",
    ] {
        assert!(place.contains(words), "{words} in {place}");
    }
}

#[test]
fn a_place_brief_opens_the_pull_request_from_the_place_with_gh() {
    let out = render(&BriefInput {
        place: true,
        ..input(None)
    });
    let place = out.split("Reports are how").next().unwrap_or_default();
    for words in [
        "git push origin HEAD",
        "from this place with gh",
        "`command -v gh`",
        "Do not commit it",
        "Name the files you saved in your final message",
    ] {
        assert!(place.contains(words), "{words} in {place}");
    }
    assert!(out.contains("install -D /tmp/"), "{out}");
}

#[test]
fn a_place_brief_claims_no_401_on_reads_of_other_repos() {
    let out = render(&BriefInput {
        place: true,
        ..input(None)
    });
    assert!(
        !out.contains("as it does for repos outside this Box"),
        "{out}"
    );
    assert!(
        out.contains("Reads of any repo carry the credential"),
        "{out}"
    );
    assert!(out.contains("A 401 or 404 on a write"), "{out}");
}

#[test]
fn names_the_issue() {
    let out = render(&input(Some("x is broken")));
    assert!(out.contains("# Box t1\n\nIssue: x is broken\n\n"));
}

#[test]
fn names_no_moor_remote_and_no_shallow_history() {
    let out = render(&input(None));
    assert!(!out.contains("shallow"), "{out}");
    assert!(!out.contains(" moor`"), "{out}");
}

#[test]
fn setup_failure_is_named_only_when_it_failed() {
    assert!(!render(&input(None)).contains("Setup failed"));
    let mut b = input(None);
    b.setup_exit = Some(3);
    let out = render(&b);
    assert!(out.contains("Setup failed (exit 3)"));
    assert!(out.contains(".moor/setup.log"));
    assert!(out.contains(".moor/reports/<kind>.md"));
}

#[test]
fn no_repo_notes_no_heading() {
    let out = render(&input(None));
    assert!(!out.contains("## This project's Box"), "{out}");
}

#[test]
fn states_cpus_and_memory_when_known() {
    let mut b = input(None);
    b.shape = Some(Shape {
        cpus: 4,
        mem_mib: 7900,
    });
    let out = render(&b);
    assert!(
        out.contains("This Box has 4 vCPUs and 7.7 GiB of memory."),
        "{out}"
    );
    b.shape = Some(Shape {
        cpus: 1,
        mem_mib: 2048,
    });
    assert!(render(&b).contains("1 vCPU and 2.0 GiB of memory."));
    assert!(!render(&input(None)).contains("vCPU"));
}

#[test]
fn names_no_host_paths_moor_commands_or_secrets() {
    let mut b = input(Some("i"));
    b.setup_exit = Some(1);
    let out = render(&b);
    assert!(!out.contains("/home/"));
    assert!(!out.contains("moor run"));
    assert!(!out.contains("moor report"));
    assert!(!out.to_lowercase().contains("token"));
}

#[test]
fn a_place_brief_offers_a_work_box_without_setup_and_a_work_brief_does_not() {
    let place = render(&BriefInput {
        place: true,
        ..input(None)
    });
    assert!(place.contains("pass `no_setup` to `start_box`"), "{place}");
    assert!(!render(&input(None)).contains("no_setup"));
}

/// Sentences of the task and run Box texts the contract replaced, one from each deleted part.
const OLD_TEXT: &[&str] = &[
    "You are running inside the machine",
    "Report the command's real exit code",
    "`sudo -n` may work here",
    "If a tool the work needs is missing, install it",
    "Before you tell the person a tool or program is missing",
    "Do not add Co-Authored-By trailers",
    "When you opened a pull request, end your final message with its URL.",
    "You may receive follow-up messages (steers)",
    "Decide on your own when you reasonably can",
    "Reports are how you show work to a human",
    "To show a human a file",
    "attach a published file to a pull request",
    "Do not run `gh auth login`",
    "do not retry or look for a way around it",
    "After each write outside moor, such as a pull request comment",
    "Report only the gate that is in hand",
    "load it first if your tools",
    "moor tells you when each gate is in hand",
    "How the run ends is not yours to say",
    "Never fake evidence or a result",
    "If you find there is nothing to do",
    "Or call it with result fail",
];

fn run_gates() -> moor_core::gate::Gates {
    let doc: moor_core::gate::GatesDoc = serde_json::from_value(serde_json::json!({
        "gates": [{"name": "proof", "evidence": ["proof.md"], "checks": ["true"]}],
        "outcomes": [
            {"name": "nothing to do", "checks": ["true"], "anytime": true},
            {"name": "done", "checks": ["true"]}
        ]
    }))
    .unwrap();
    moor_core::gate::Gates::parse(&doc.gates, &doc.outcomes).unwrap()
}

#[test]
fn no_old_task_or_run_box_text_is_reachable() {
    let run = RunLaunch {
        link: Some("https://moor.example/#/runs/7".to_owned()),
    };
    let gates = run_gates();
    let proof = gates.get(0).unwrap();
    let texts = [
        render(&input(Some("i"))),
        render(&BriefInput {
            run: Some(&run),
            setup_exit: Some(1),
            ..input(None)
        }),
        gates.first_message("Collect evidence."),
        proof.remind_steer(),
        gates.none_held_steer(&[]),
    ];
    for text in &texts {
        for old in OLD_TEXT {
            assert!(!text.contains(old), "{old:?} is still said:\n{text}");
        }
    }
}

#[test]
fn a_task_box_reads_the_every_and_box_sections_then_its_facts() {
    let out = render(&input(Some("x is broken")));
    assert!(
        out.starts_with("# moor agent contract\n\nVersion 1\n\n"),
        "{out}"
    );
    assert!(out.contains("## every\n\n- [secrets]") && out.contains("## box\n\n- [branch]"));
    assert!(
        !out.contains("## run") && !out.contains("report_step"),
        "{out}"
    );
    let facts = out.find("# Box t1\n\nIssue: x is broken\n\n").unwrap();
    assert!(out.find("## box").unwrap() < facts, "{out}");
}

#[test]
fn a_run_box_adds_the_run_section_and_its_link_to_the_facts() {
    let run = RunLaunch {
        link: Some("https://moor.example/#/runs/7".to_owned()),
    };
    let out = render(&BriefInput {
        run: Some(&run),
        ..input(None)
    });
    assert!(out.contains("## run\n\n- [unattended]"), "{out}");
    assert!(out.contains("report_output, report_step"), "{out}");
    assert!(
        out.contains("- This run's link: https://moor.example/#/runs/7\n"),
        "{out}"
    );
    let unlinked = RunLaunch { link: None };
    let out = render(&BriefInput {
        run: Some(&unlinked),
        ..input(None)
    });
    assert!(!out.contains("run's link:"), "{out}");
}

#[test]
fn the_facts_name_the_branch_its_push_line_and_the_folders() {
    let out = render(&input(None));
    let facts = out.split("# Box t1").nth(1).unwrap();
    for fact in [
        "on branch `moor/t1`",
        "`git push origin HEAD:refs/heads/moor/t1`",
        "partial clone",
        "artifacts folder is `.moor/artifacts/`",
        "reports folder is `.moor/reports/`",
    ] {
        assert!(facts.contains(fact), "{fact} in {facts}");
    }
}

#[test]
fn repo_notes_are_facts_after_the_contract_and_the_issue() {
    let mut b = input(Some("x is broken"));
    b.repo_notes = Some("Start the emulator:\n  make emu\n");
    let out = render(&b);
    let notes =
        "## This project's Box\n\nFrom the repo's config:\n\nStart the emulator:\n  make emu\n\n";
    let at = out.find(notes).unwrap();
    assert!(out.find("Issue: x is broken").unwrap() < at, "{out}");
}

#[test]
fn the_skills_and_tools_are_listed_only_when_the_launch_has_them() {
    let skills = [SkillEntry {
        name: "moor-attach-files",
        description: "Show a file on a pull request.",
        path: "/box/cred/skills/moor-attach-files/SKILL.md".to_owned(),
    }];
    let out = render(&BriefInput {
        skills: &skills,
        ..input(None)
    });
    assert!(out.contains("- moor-attach-files: Show a file on a pull request. Its file: `/box/cred/skills/moor-attach-files/SKILL.md`"), "{out}");
    let bare = render(&BriefInput {
        moor_tools: false,
        ..input(None)
    });
    assert!(
        !bare.contains("## moor tools") && !bare.contains("## moor skills"),
        "{bare}"
    );
}

#[test]
fn a_box_without_gh_gets_the_pinned_recipe_as_a_fact() {
    let out = render(&input(None));
    let recipe = out
        .split("```sh\n")
        .nth(1)
        .and_then(|s| s.split("```").next())
        .unwrap();
    let check = std::process::Command::new("bash")
        .args(["-n", "-c", recipe])
        .status()
        .unwrap();
    assert!(check.success(), "{recipe}");
    assert!(recipe.contains("sha256sum -c"), "{recipe}");
}
