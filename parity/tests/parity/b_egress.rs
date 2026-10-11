use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use parity::adapter::{EnvOpts, EnvReport, ExitClass, Outcome, StartOpts, TaskView};
use parity::fake::crabbox::{self, Preset};
use parity::fake::{claude, herdr, Call, Rule};
use parity::parity;
use parity::world::repo::Repo;
use parity::world::World;
use serde_json::{json, Value};

use crate::{moor, world};

const BASE: [&str; 17] = [
    "github.com",
    "api.github.com",
    "*.githubusercontent.com",
    "registry.npmjs.org",
    "mise.jdx.dev",
    "mise-versions.jdx.dev",
    "mise.run",
    "nodejs.org",
    "api.anthropic.com",
    "registry-1.docker.io",
    "auth.docker.io",
    "production.cloudflare.docker.com",
    "download.docker.com",
    "deb.debian.org",
    "security.debian.org",
    "archive.ubuntu.com",
    "security.ubuntu.com",
];
const PROXY_SERVED: [&str; 2] = ["api.github.com", "api.anthropic.com"];
const NS_ENV: &str = "CRABBOX_NAMESPACE_EGRESS_DOMAINS";
const SPRITES_FLAG: &str = "--sprites-network-allow";

struct Fx {
    w: World,
    repo: Repo,
}

fn fx(preset: Preset, enforce: bool, user_extra: Value, repo_cfg: Value) -> Fx {
    let w = world();
    let repo = Repo::create(&w, "widget");
    let mut bx = json!({ "provider": preset.provider() });
    if enforce {
        bx["enforce_egress"] = json!(true);
    }
    let mut user = json!({ "box": bx });
    for (k, v) in user_extra.as_object().into_iter().flatten() {
        user[k] = v.clone();
    }
    std::fs::write(w.config_path(), user.to_string()).expect("user config");
    std::fs::write(repo.path.join(".moor.yaml"), repo_cfg.to_string()).expect("repo config");
    crabbox::install_preset(&w, preset);
    herdr::install(&w);
    claude::install(&w);
    crabbox::ssh_channel(&w);
    crabbox::tunnel(&w);
    w.add_rules(
        "ssh",
        [Rule::any()
            .exec("cd '{box}' && {last}")
            .rewrite("/tmp/", "{box}/tmp/")
            .capture_stdin()],
    );
    Fx { w, repo }
}

fn cfg(location: &str, egress: &[&str]) -> Value {
    json!({ "agent": { "location": location }, "egress": egress })
}

impl Fx {
    fn set_repo(&self, doc: Value) {
        std::fs::write(self.repo.path.join(".moor.yaml"), doc.to_string()).expect("repo config");
    }

    fn start(&self, base: &str, goal: Option<&str>) -> (String, Outcome<TaskView>) {
        self.start_with(base, goal, None)
    }

    fn start_with(
        &self,
        base: &str,
        goal: Option<&str>,
        issue: Option<&str>,
    ) -> (String, Outcome<TaskView>) {
        let name = self.w.task_name(base);
        let opts = StartOpts {
            goal: goal.map(str::to_string),
            issue: issue.map(str::to_string),
            ..Default::default()
        };
        let o = moor().start(&self.w, &self.repo.path, &name, &opts);
        (name, o)
    }

    fn start_ok(&self, base: &str, goal: Option<&str>) -> String {
        let (name, o) = self.start(base, goal);
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        name
    }

    fn warmups(&self) -> Vec<Call> {
        self.w
            .calls("crabbox")
            .into_iter()
            .filter(|c| c.argv.first().map(String::as_str) == Some("warmup"))
            .collect()
    }

    fn warmup(&self) -> Call {
        let mut all = self.warmups();
        assert_eq!(all.len(), 1, "exactly one warmup");
        all.remove(0)
    }

    fn finish(&self, name: &str) {
        let closed = moor().close(&self.w, &self.repo.path, name, false);
        assert_eq!(closed.class, ExitClass::Ok, "{}", closed.stderr);
        let dir = self.w.state_dir().join("proxy");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        while std::time::Instant::now() < deadline {
            let live = std::fs::read_dir(&dir).is_ok_and(|d| {
                d.flatten()
                    .any(|e| e.path().extension().is_some_and(|x| x == "json"))
            });
            if !live {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        panic!("proxy supervisor did not exit after close");
    }
}

fn csv(raw: &str) -> Vec<String> {
    raw.split(',').map(str::to_string).collect()
}

fn sprites_list(c: &Call) -> Option<Vec<String>> {
    c.flag(SPRITES_FLAG).map(|v| csv(&v))
}

fn ns_list(c: &Call) -> Option<Vec<String>> {
    c.env.get(NS_ENV).map(|v| csv(v))
}

fn assert_set(list: &[String], expected: &BTreeSet<String>) {
    let got: BTreeSet<String> = list.iter().cloned().collect();
    assert_eq!(got.len(), list.len(), "duplicates in {list:?}");
    assert_eq!(&got, expected, "allow list differs");
}

fn want(base_without: &[&str], extra: &[&str]) -> BTreeSet<String> {
    BASE.iter()
        .filter(|h| !base_without.contains(h))
        .chain(extra.iter())
        .map(|s| s.to_string())
        .collect()
}

fn no_policy_anywhere(w: &World) {
    for c in w.calls("crabbox") {
        assert!(c.flag(SPRITES_FLAG).is_none(), "{:?}", c.argv);
        assert!(!c.env.contains_key(NS_ENV), "{:?}", c.argv);
    }
}

parity!(
    egress_entries_are_validated_at_start,
    covers = ["B-98"],
    tags = [],
    {
        let f = fx(Preset::Sprites, false, json!({}), json!({}));
        let bad = [
            "1.1.1.1",
            "https://x.example.com",
            "x.example.com/path",
            "x.example.com:443",
            "*",
            "localhost",
            "*.",
            "*.*.example.com",
        ];
        for entry in bad {
            f.set_repo(cfg("host", &[entry]));
            let (_, o) = f.start("bad", None);
            assert_ne!(o.class, ExitClass::Ok, "{entry} must be rejected");
            assert!(o.stderr.contains(entry), "{entry} not named: {}", o.stderr);
        }
        assert!(f.warmups().is_empty(), "no Box acquired for a bad entry");
        f.set_repo(cfg("host", &["Example.COM"]));
        let name = f.start_ok("good", None);
        let list = sprites_list(&f.warmup()).expect("allow flag");
        assert!(list.contains(&"example.com".to_string()), "{list:?}");
        assert!(!list.contains(&"Example.COM".to_string()), "{list:?}");
        f.finish(&name);
    }
);

parity!(
    allow_list_is_base_plus_repo_entries_deduplicated,
    covers = ["B-99", "C-121", "C-122"],
    tags = [],
    {
        let entries = [
            "Example.com",
            "*.Example.com",
            "example.com",
            "a.example.com",
            "github.com",
        ];
        let f = fx(Preset::Sprites, false, json!({}), cfg("host", &entries));
        let name = f.start_ok("base", None);
        let list = sprites_list(&f.warmup()).expect("allow flag");
        assert_set(
            &list,
            &want(&[], &["example.com", "*.example.com", "a.example.com"]),
        );
        f.finish(&name);
    }
);

parity!(
    proxy_active_box_agent_cannot_reach_api_hosts_directly,
    covers = ["B-99", "C-122"],
    tags = [],
    {
        let f = fx(
            Preset::Sprites,
            false,
            json!({}),
            cfg("box", &["a.example.com"]),
        );
        let name = f.start_ok("px", None);
        let list = sprites_list(&f.warmup()).expect("allow flag");
        assert_set(&list, &want(&PROXY_SERVED, &["a.example.com"]));
        for kept in [
            "github.com",
            "*.githubusercontent.com",
            "mise.run",
            "nodejs.org",
        ] {
            assert!(list.contains(&kept.to_string()), "{kept} dropped");
        }
        f.finish(&name);

        for served in ["api.github.com", "API.Anthropic.com"] {
            f.set_repo(cfg("box", &[served]));
            let (_, o) = f.start("rej", None);
            assert_ne!(o.class, ExitClass::Ok, "{served} must be refused");
            assert!(
                o.stderr.contains(served),
                "{served} not named: {}",
                o.stderr
            );
        }
        assert_eq!(f.warmups().len(), 1, "no Box acquired for refused entries");

        f.set_repo(cfg("host", &["api.github.com"]));
        let name = f.start_ok("hostok", None);
        let list = sprites_list(f.warmups().last().expect("warmup")).expect("allow flag");
        assert_set(&list, &want(&[], &[]));
        f.finish(&name);
    }
);

parity!(
    sprites_policy_is_on_by_default_for_box_agents_only,
    covers = ["B-100", "C-123"],
    tags = [],
    {
        let boxed = fx(Preset::Sprites, false, json!({}), cfg("box", &[]));
        let name = boxed.start_ok("bx", None);
        let list = sprites_list(&boxed.warmup()).expect("flag despite empty egress");
        assert_set(&list, &want(&PROXY_SERVED, &[]));
        boxed.finish(&name);

        let host = fx(Preset::Sprites, false, json!({}), cfg("host", &[]));
        let name = host.start_ok("hs", None);
        no_policy_anywhere(&host.w);
        host.finish(&name);

        let ns = fx(Preset::NamespaceDevbox, false, json!({}), cfg("box", &[]));
        let name = ns.start_ok("nsbx", None);
        no_policy_anywhere(&ns.w);
        ns.finish(&name);
    }
);

parity!(
    policy_warmup_failure_fails_start_without_fallback,
    covers = ["B-102", "C-123"],
    tags = [],
    {
        let f = fx(
            Preset::Sprites,
            false,
            json!({}),
            cfg("host", &["a.example.com"]),
        );
        crabbox::fail_quota(&f.w);
        let (_, o) = f.start("fail", None);
        assert_ne!(o.class, ExitClass::Ok, "{}", o.stdout);
        let warm = f.warmups();
        assert_eq!(warm.len(), 1, "no retry without the policy");
        assert!(sprites_list(&warm[0]).is_some());
        assert!(f
            .w
            .calls("crabbox")
            .iter()
            .all(|c| c.argv.first().map(String::as_str) != Some("run")));
    }
);

parity!(
    other_providers_apply_no_egress_policy,
    covers = ["B-102", "C-125"],
    tags = [],
    {
        for preset in [Preset::LocalContainer, Preset::Ssh] {
            for location in ["host", "box"] {
                let f = fx(preset, true, json!({}), cfg(location, &["a.example.com"]));
                let name = f.start_ok("oth", None);
                f.warmup();
                no_policy_anywhere(&f.w);
                f.finish(&name);
            }
        }
    }
);

parity!(
    namespace_policy_needs_opt_in_and_matches_sprites_list,
    covers = ["B-101", "C-124"],
    tags = [],
    {
        let entries = ["a.example.com"];
        for location in ["host", "box"] {
            let ns = fx(
                Preset::NamespaceDevbox,
                true,
                json!({}),
                cfg(location, &entries),
            );
            let name = ns.start_ok("nsop", None);
            let warm = ns.warmup();
            assert!(warm.flag(SPRITES_FLAG).is_none());
            let ns_set = ns_list(&warm).expect("egress env with opt-in");
            ns.finish(&name);

            let sp = fx(Preset::Sprites, false, json!({}), cfg(location, &entries));
            let name = sp.start_ok("spcmp", None);
            let sp_set = sprites_list(&sp.warmup()).expect("flag");
            sp.finish(&name);
            assert_eq!(ns_set, sp_set, "{location}: same list as sprites");
        }

        let off = fx(
            Preset::NamespaceDevbox,
            false,
            json!({}),
            cfg("host", &entries),
        );
        let (name, o) = off.start("nsoff", None);
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        assert!(!o.warnings.is_empty(), "unenforced namespace start warns");
        assert!(ns_list(&off.warmup()).is_none());
        off.finish(&name);

        let sp = fx(Preset::Sprites, true, json!({}), cfg("host", &entries));
        let (name, o) = sp.start("spen", None);
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        assert!(o.warnings.is_empty(), "{:?}", o.warnings);
        let warm = sp.warmup();
        assert!(!warm.env.contains_key(NS_ENV));
        assert!(sprites_list(&warm).is_some());
        sp.finish(&name);
    }
);

fn egress_row(o: &Outcome<EnvReport>) -> (String, String) {
    let rep = o.ok();
    let rows = rep.of_kind("egress");
    let main: Vec<_> = rows.iter().filter(|i| i.name != "enforce").collect();
    assert_eq!(main.len(), 1, "one egress row besides the enforce note");
    (
        main[0].name.clone(),
        main[0].detail.clone().unwrap_or_default(),
    )
}

fn env_at(f: &Fx, location: &str) -> Outcome<EnvReport> {
    let opts = EnvOpts {
        agent_location: Some(location.to_string()),
        ..Default::default()
    };
    moor().env(&f.w, &f.repo.path, &opts)
}

parity!(
    env_egress_item_follows_provider_capability,
    covers = ["B-102a"],
    tags = [],
    {
        let f = fx(
            Preset::Sprites,
            false,
            json!({}),
            cfg("host", &["a.example.com"]),
        );
        let (name, detail) = egress_row(&env_at(&f, "host"));
        assert_eq!(name, "allow");
        assert!(detail.contains("a.example.com"), "{detail}");
        let plain = moor().env(&f.w, &f.repo.path, &EnvOpts::default());
        let out = plain.stdout.to_lowercase();
        assert!(
            !out.contains("crabbox") && !out.contains("network-allow"),
            "{out}"
        );

        let (name, detail) = egress_row(&env_at(&f, "box"));
        assert_eq!(name, "allow");
        assert!(detail.contains("github.com"), "{detail}");
        assert!(!detail.contains("api.anthropic.com"), "{detail}");

        f.set_repo(cfg("host", &[]));
        let (open_name, open) = egress_row(&env_at(&f, "host"));
        assert_eq!(open_name, "policy");
        let (box_name, _) = egress_row(&env_at(&f, "box"));
        assert_eq!(box_name, "allow", "Box agent gets a policy even when empty");

        let lc = fx(Preset::LocalContainer, false, json!({}), cfg("host", &[]));
        let (lc_name, none) = egress_row(&env_at(&lc, "host"));
        assert_eq!(lc_name, "policy");
        assert_ne!(open, none, "open and unsupported read differently");
        lc.set_repo(cfg("host", &["a.example.com", "b.example.com"]));
        let (_, with_entries) = egress_row(&env_at(&lc, "host"));
        assert_ne!(with_entries, none);
        assert!(with_entries.contains('2'), "{with_entries}");

        let ns = fx(
            Preset::NamespaceDevbox,
            true,
            json!({}),
            cfg("host", &["a.example.com"]),
        );
        let (ns_name, ns_detail) = egress_row(&env_at(&ns, "host"));
        assert_eq!(ns_name, "allow");
        assert!(ns_detail.contains("a.example.com"));
        assert!(env_at(&ns, "host").ok().item("egress", "enforce").is_none());
        let off = fx(
            Preset::NamespaceDevbox,
            false,
            json!({}),
            cfg("host", &["a.example.com"]),
        );
        let (off_name, _) = egress_row(&env_at(&off, "host"));
        assert_eq!(off_name, "policy");
        assert!(env_at(&off, "host")
            .ok()
            .item("egress", "enforce")
            .is_some());
    }
);

fn files_named(dir: &Path, name: &str, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            files_named(&p, name, out);
        } else if p.file_name().is_some_and(|n| n == name) {
            out.push(p);
        }
    }
}

fn box_file(w: &World, name: &str) -> Option<String> {
    let mut found = Vec::new();
    files_named(&w.box_root(), name, &mut found);
    assert!(found.len() <= 1, "several {name}: {found:?}");
    found.first().and_then(|p| std::fs::read_to_string(p).ok())
}

fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_ascii_alphanumeric())
        .map(str::to_lowercase)
        .collect()
}

fn branch_of(w: &World) -> String {
    w.calls("herdr")
        .iter()
        .find(|c| {
            c.argv
                .starts_with(&["worktree".to_string(), "create".to_string()])
        })
        .and_then(|c| c.flag("--branch"))
        .expect("worktree branch")
}

fn box_brief_fx(repo_cfg: Value) -> Fx {
    let mut c = repo_cfg;
    c["agent"] = json!({ "location": "box" });
    let f = fx(Preset::Sprites, false, json!({}), c);
    let file = f.w.root().join("claude-token");
    std::fs::write(&file, format!("sk-parity-{}", f.w.task_name("tok"))).expect("token file");
    let user = json!({
        "box": { "provider": "sprites" },
        "credentials": { "CLAUDE_CODE_OAUTH_TOKEN": { "file": file } },
    });
    std::fs::write(f.w.config_path(), user.to_string()).expect("user config");
    f
}

fn token_of(f: &Fx) -> String {
    std::fs::read_to_string(f.w.root().join("claude-token")).expect("token")
}

parity!(
    box_brief_states_the_box_contract_and_carries_no_secret,
    covers = ["C-139"],
    tags = [],
    {
        let f = box_brief_fx(json!({}));
        let (name, o) = f.start_with("brf", Some("ship the widget"), Some("ABC-7"));
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        let brief = box_file(&f.w, "brief.md").expect("brief delivered to the Box");
        let branch = branch_of(&f.w);
        assert!(brief.contains("Goal: ship the widget"), "{brief}");
        assert!(brief.contains("Issue: ABC-7"), "{brief}");
        for marker in [
            "sudo",
            "official",
            "detached",
            "$MOOR_PUSH_URL",
            "Co-Authored-By",
            "commit-msg hook",
            ".moor/reports/",
            "exit code",
            "tail",
        ] {
            assert!(brief.contains(marker), "missing marker {marker}");
        }
        assert!(
            brief.contains(&format!("HEAD:refs/heads/{branch}")),
            "push target names the Task branch {branch}"
        );
        assert!(
            !brief.contains("Setup failed"),
            "no failure note on success"
        );

        assert!(!brief.contains(&token_of(&f)), "token in brief");
        if let Some(nonce) = box_file(&f.w, "proxy-nonce") {
            assert!(!brief.contains(nonce.trim()), "nonce in brief");
        }
        for host_side in [
            f.w.root().to_string_lossy().into_owned(),
            f.repo.path.to_string_lossy().into_owned(),
        ] {
            assert!(!brief.contains(&host_side), "host path in brief");
        }
        for sub in ["run", "report", "steer", "cp", "gate", "close", "proxy"] {
            assert!(
                !brief.contains(&format!("moor {sub}")),
                "moor {sub} in brief"
            );
        }
        let w = words(&brief);
        assert!(!w.contains(&"gh".to_string()), "brief mentions gh");
        assert!(
            !w.contains(&"proxy".to_string()),
            "brief mentions the proxy"
        );
        f.finish(&name);
    }
);

parity!(
    box_brief_reports_setup_failure_only_when_failed,
    covers = ["C-139"],
    tags = [],
    {
        let f = box_brief_fx(json!({ "setup": ["exit 3"] }));
        let (name, o) = f.start("sfail", Some("go"));
        assert_ne!(o.class, ExitClass::Usage, "{}", o.stderr);
        let brief = box_file(&f.w, "brief.md").expect("brief delivered despite setup failure");
        assert!(brief.contains("Setup failed (exit 3)"), "{brief}");
        assert!(brief.contains(".moor/setup.log"), "{brief}");
        f.finish(&name);
    }
);

fn bg_call(w: &World) -> Call {
    w.calls("claude")
        .into_iter()
        .find(|c| c.argv.iter().any(|a| a == "--bg"))
        .expect("claude --bg dispatch")
}

fn allowed_tools(c: &Call) -> Vec<String> {
    let at = c.argv.iter().position(|a| a == "--allowedTools");
    let rest = at.map(|i| &c.argv[i + 1..]).unwrap_or_default();
    rest.iter()
        .take_while(|a| !a.starts_with("--"))
        .cloned()
        .collect()
}

parity!(
    host_brief_routes_tooling_through_run_and_forbids_push,
    covers = ["C-140"],
    tags = [],
    {
        let f = fx(Preset::Sprites, false, json!({}), cfg("host", &[]));
        let name = f.start_ok("hbf", Some("ship the widget"));
        let call = bg_call(&f.w);
        let brief = call.argv[0].clone();
        let branch = branch_of(&f.w);
        assert!(brief.contains("Goal: ship the widget"), "{brief}");
        assert!(
            brief.contains(&format!(" run {name} -- <command>")),
            "{brief}"
        );
        assert!(brief.contains("ONLY way"), "{brief}");
        assert!(brief.contains(&format!("branch `{branch}`")), "{brief}");
        for marker in ["Do not push", "Do not switch branches", "git -C", "tail"] {
            assert!(brief.contains(marker), "missing marker {marker}");
        }
        assert!(brief.contains(&format!(" report {name} --kind <kind>")));
        assert!(brief.contains(".moor/reports/<kind>.md"));
        assert!(
            !brief.contains("publish_artifact"),
            "default backend is local"
        );
        let tools = allowed_tools(&call);
        assert!(!tools.is_empty(), "allowed tools listed");
        for t in &tools {
            assert!(!t.starts_with("Bash(git push"), "{t}");
            assert!(!t.starts_with("Bash(gh"), "{t}");
        }
        f.finish(&name);
    }
);

parity!(
    host_brief_publishes_reports_when_artifacts_backend_is_set,
    covers = ["C-140"],
    tags = [],
    {
        let f = fx(
            Preset::Sprites,
            false,
            json!({ "reports": { "backend": "agentic-artifacts" } }),
            cfg("host", &[]),
        );
        let name = f.start_ok("hart", Some("ship the widget"));
        let brief = bg_call(&f.w).argv[0].clone();
        for marker in [
            "publish_artifact",
            "commit_artifact",
            "sha256sum",
            "--link",
            "--version",
        ] {
            assert!(brief.contains(marker), "missing marker {marker}");
        }
        assert!(brief.contains("fall back"), "local fallback is stated");
        f.finish(&name);
    }
);

parity!(
    start_without_goal_dispatches_no_agent_and_no_proxy,
    covers = ["C-141"],
    tags = [],
    {
        for location in ["host", "box"] {
            let f = fx(Preset::Sprites, false, json!({}), cfg(location, &[]));
            let (name, o) = f.start_with("nogoal", None, Some("ABC-7"));
            assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
            assert!(
                f.w.calls("claude").is_empty(),
                "{location}: agent dispatched"
            );
            assert!(box_file(&f.w, "brief.md").is_none(), "{location}: brief");
            assert!(
                f.w.calls("crabbox")
                    .iter()
                    .all(|c| c.argv.first().map(String::as_str) != Some("tunnel")),
                "{location}: tunnel opened"
            );
            let proxy_dir = f.w.state_dir().join("proxy");
            let proxies = std::fs::read_dir(proxy_dir).map_or(0, |d| d.flatten().count());
            assert_eq!(proxies, 0, "{location}: proxy state written");
            f.finish(&name);
        }
    }
);

parity!(
    goal_and_model_reach_the_dispatched_host_agent,
    covers = ["C-141"],
    tags = [],
    {
        let f = fx(Preset::Sprites, false, json!({}), cfg("host", &[]));
        let name = f.w.task_name("mdl");
        let opts = StartOpts {
            goal: Some("tune the widget".to_string()),
            model: Some("sonnet".to_string()),
            ..Default::default()
        };
        let o = moor().start(&f.w, &f.repo.path, &name, &opts);
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        let call = bg_call(&f.w);
        assert_eq!(call.flag("--model").as_deref(), Some("sonnet"));
        assert!(call.argv[0].contains("Goal: tune the widget"));
        f.finish(&name);
    }
);
