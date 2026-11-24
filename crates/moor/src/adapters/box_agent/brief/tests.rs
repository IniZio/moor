//! Tests for the brief.

use super::*;

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
fn names_no_prompt_or_goal_line() {
    let out = render(&input(None));
    assert!(
        out.starts_with("# Box t1\n\nYou are running inside the machine"),
        "{out}"
    );
    assert!(!out.contains("Goal:") && !out.contains("Prompt:"), "{out}");
}

#[test]
fn names_the_issue() {
    let out = render(&input(Some("x is broken")));
    assert!(out.contains("# Box t1\n\nIssue: x is broken\n\n"));
}

#[test]
fn states_the_box_contract() {
    let out = render(&input(None));
    assert!(out.contains("git push origin HEAD:refs/heads/moor/t1"));
    assert!(out.contains("on branch `moor/t1`"));
    assert!(out.contains("partial clone"));
    assert!(!out.contains("moor HEAD"), "{out}");
    assert!(out.contains("Do not add Co-Authored-By trailers"));
    assert!(out.contains("follow-up messages (steers)"));
}

#[test]
fn asks_for_the_pull_request_url_at_the_end_of_the_final_message() {
    let out = render(&input(None));
    assert!(
        out.contains("When you opened a pull request, end your final message with its URL."),
        "{out}"
    );
}

#[test]
fn asks_only_when_the_choice_is_the_owners() {
    let out = render(&input(None));
    assert!(out.contains(
        "Decide on your own when you reasonably can, and ask a question only when the choice is the owner's.\n\n"
    ));
}

#[test]
fn names_no_moor_remote_and_no_shallow_history() {
    let out = render(&input(None));
    assert!(!out.contains("shallow"), "{out}");
    assert!(!out.contains(" moor`"), "{out}");
}

#[test]
fn says_how_to_attach_artifacts_to_a_pull_request_right_after_publishing() {
    let attach = "When gh is on the PATH, attach a published file to a pull request with `gh pr create --attach .moor/artifacts/<name>` or `gh pr edit <number> --attach .moor/artifacts/<name>`. Write `![<name>](.moor/artifacts/<name>)` in the body where it should appear. gh replaces it with the uploaded URL. gh attaches png, jpg, jpeg, gif, webp, svg, mp4, mov and webm files: images up to 10 MB, videos up to 100 MB on paid plans, at most 50 files. An upload cannot be deleted.\n\n";
    let out = render(&input(None));
    assert!(
        out.contains(&format!("in your final message.\n\n{attach}")),
        "{out}"
    );
}

#[test]
fn says_how_to_install_a_pinned_gh_that_signs_in_through_the_proxy() {
    let out = render(&input(None));
    let recipe = out
        .split("```sh\n")
        .nth(1)
        .and_then(|s| s.split("```").next())
        .expect("an sh block");
    let check = std::process::Command::new("bash")
        .args(["-n", "-c", recipe])
        .status()
        .expect("bash");
    assert!(check.success(), "{recipe}");
    for arch in ["amd64", "arm64"] {
        assert!(
            recipe.contains(&format!("  gh_2.105.0_linux_{arch}.tar.gz'")),
            "{recipe}"
        );
    }
    assert!(recipe.contains("sha256sum -c"), "{recipe}");
    assert!(out.contains("Do not run `gh auth login`"), "{out}");
}

#[test]
fn says_how_to_publish_artifacts() {
    let out = render(&input(None));
    assert!(out.contains(".moor/artifacts/"));
    assert!(out.contains("Do not commit it"));
    assert!(out.contains("adds a new version"));
    assert!(out.contains("when you finish your turn"));
    assert!(out.contains("Name the files you published in your final message"));
}

#[test]
fn states_limits_and_points_missing_tools_at_setup() {
    let out = render(&input(None));
    assert!(out.contains(
        "`sudo -n` may work here: test it before you say you lack access, and use it when \
         the project's tools need it. Anything later Boxes also need belongs in the repo's \
         setup commands."
    ));
    assert!(!out.contains("Do not use sudo"));
    assert!(!out.contains("sandbox"));
    assert!(out.contains("The network is open."));
    assert!(out.contains("When HTTPS_PROXY is set"));
    assert!(out.contains("Write only in the working directory"));
    assert!(out.contains("expect 401 or 404"));
    assert!(out.contains("Do not guess paths."));
}

#[test]
fn tells_the_agent_to_install_a_missing_tool() {
    let out = render(&input(None));
    assert!(out.contains("If a tool the work needs is missing, install it"));
    assert!(!out.contains("do not try to install"));
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
fn repo_notes_follow_the_issue_under_their_own_heading_verbatim() {
    let mut b = input(Some("x is broken"));
    b.repo_notes = Some("Start the emulator:\n  make emu\n");
    let out = render(&b);
    let issue = out.find("Issue: x is broken").unwrap();
    let heading =
        "## This project's Box\n\nFrom the repo's config:\n\nStart the emulator:\n  make emu\n\n";
    assert!(out.contains(heading), "{out}");
    let head = out.find(heading).unwrap();
    assert!(issue < head, "{out}");
    assert!(head < out.find("You are running inside the machine").unwrap());
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
fn says_to_check_before_calling_a_tool_missing() {
    let out = render(&input(None));
    assert!(
        out.contains(
            "Before you tell the person a tool or program is missing, check for it \
             (for example `command -v <name>`, or the desktop's applications) and say what \
             you checked."
        ),
        "{out}"
    );
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
