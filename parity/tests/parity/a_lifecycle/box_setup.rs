use std::path::Path;

use parity::adapter::{BoxState, EventKind, ExitClass, StartOpts, TaskState};
use parity::assert::{assert_never_called, assert_pure_json};
use parity::fake::{crabbox, Rule};
use parity::parity;
use parity::world::config::UserConfig;
use serde_json::json;

use super::support::*;

parity!(
    box_credential_preflight_failure_acquires_no_box,
    covers = ["A-20", "REQ-18"],
    tags = [],
    {
        let f = fx();
        f.repo_cfg(json!({ "agent": { "location": "box" } }));
        git_in(
            &f,
            &f.repo.path,
            &[
                "remote",
                "set-url",
                "--push",
                "origin",
                "https://github.com/acme/gadget",
            ],
        );
        let name = f.name("pre");
        let o = f.try_start(&name, &StartOpts::default());
        assert!(o.class.is_err());
        assert_eq!(f.row(&name).state, TaskState::SetupFailed);
        assert!(f.kinds(&name).contains(&EventKind::CredentialFailed));
        assert_never_called(&f.w.all_calls(), "crabbox", &["warmup"]);
        f.repo_cfg(json!({ "agent": { "location": "host" } }));
        let host = f.name("host");
        assert_eq!(f.start(&host).state, TaskState::Working);
    }
);

parity!(
    namespace_without_enforce_egress_warns_at_start,
    covers = ["A-21", "REQ-42"],
    tags = [],
    {
        let f = fx_preset(crabbox::Preset::NamespaceDevbox, UserConfig::default());
        let o = f.try_start(&f.name("ns"), &StartOpts::default());
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        assert!(!o.warnings.is_empty(), "no warning: {}", o.stdout);
        let local = fx();
        let quiet = local.try_start(&local.name("lc"), &StartOpts::default());
        assert!(quiet.warnings.is_empty(), "{:?}", quiet.warnings);
    }
);

parity!(
    driver_chatter_never_corrupts_json_or_leaks_vocabulary,
    covers = ["A-22", "A-103", "REQ-22"],
    tags = [],
    {
        let f = fx();
        f.w.prepend_rules(
            "crabbox",
            [Rule::on(&["warmup"])
                .stdout("lease cbx_1 slug={flag:--slug} pond\nconsole.example\n")
                .stderr("SSH: lease slug pond\n")],
        );
        let name = f.name("json");
        let o = f.try_start(&name, &StartOpts::default());
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        let mut outs = vec![o.stdout.clone()];
        let listed = f.m.list(&f.w, &f.repo.path);
        outs.push(listed.stdout.clone());
        outs.push(f.try_close(&name).stdout);
        for out in &outs {
            assert_pure_json(out);
            let lower = out.to_lowercase();
            for word in ["lease", "slug", "pond", "console.example"] {
                assert!(!lower.contains(word), "{word} in {out}");
            }
        }
    }
);

parity!(
    start_returns_after_box_setup_and_agent_launch_in_order,
    covers = ["A-24", "A-30"],
    tags = [],
    {
        let f = fx();
        f.repo_cfg(json!({ "setup": ["echo setup-marker"], "provision": ["secret.txt"] }));
        std::fs::write(f.repo.path.join("secret.txt"), "s3").unwrap();
        let name = f.name("order");
        let t = f.goal(&name);
        assert_eq!(t.state, TaskState::Working);
        assert_eq!(
            std::fs::read_to_string(Path::new(t.worktree.as_deref().unwrap()).join("secret.txt"))
                .unwrap(),
            "s3"
        );
        let calls = f.w.all_calls();
        let warm = call_seq(&calls, |c| is_verb(c, "crabbox", "warmup"));
        let setup = call_seq(&calls, |c| {
            is_verb(c, "crabbox", "run") && c.argv.iter().any(|a| a.contains("setup-marker"))
        });
        let agent = call_seq(&calls, is_bg);
        assert!(warm < setup && setup < agent, "{warm} {setup} {agent}");
    }
);

parity!(
    box_size_selects_class_and_size_env,
    covers = ["A-26"],
    tags = [],
    {
        for (size, class, env) in [
            ("small", "standard", "S"),
            ("medium", "fast", "M"),
            ("large", "large", "L"),
            ("xlarge", "beast", "XL"),
        ] {
            let f = fx();
            f.repo_cfg(json!({ "box": { "size": size } }));
            let name = f.name("sz");
            let t = f.start(&name);
            let warm = f.verb("crabbox", "warmup").remove(0);
            let slug = warm.flag("--slug").expect("slug");
            assert!(slug.starts_with(&format!("moor-{name}-")), "{slug}");
            assert_eq!(warm.flag("--class").as_deref(), Some(class));
            assert_eq!(
                warm.env.get("CRABBOX_NAMESPACE_SIZE").map(String::as_str),
                Some(env)
            );
            assert_eq!(warm.flag("--provider").as_deref(), Some("local-container"));
            assert_eq!(
                t.boxed.unwrap().handle,
                Some(format!("local-container/{slug}"))
            );
            assert_eq!(
                std::fs::canonicalize(&warm.cwd).ok(),
                std::fs::canonicalize(t.worktree.unwrap()).ok()
            );
        }
    }
);

parity!(
    egress_policy_reaches_only_sprites_and_enforcing_namespace,
    covers = ["A-27", "REQ-42"],
    tags = [],
    {
        let egress = json!({ "egress": ["example.com"] });
        let sprites = fx_preset(crabbox::Preset::Sprites, UserConfig::default());
        sprites.repo_cfg(egress.clone());
        sprites.start(&sprites.name("sp"));
        let warm = sprites.verb("crabbox", "warmup").remove(0);
        let allow = warm.flag("--sprites-network-allow").expect("allow list");
        assert!(allow.split(',').any(|d| d == "example.com"), "{allow}");

        let local = fx();
        local.repo_cfg(egress.clone());
        local.start(&local.name("lc"));
        let warm = local.verb("crabbox", "warmup").remove(0);
        assert_eq!(warm.flag("--sprites-network-allow"), None);
        assert!(!warm.env.contains_key("CRABBOX_NAMESPACE_EGRESS_DOMAINS"));

        let ns = fx_preset(crabbox::Preset::NamespaceDevbox, UserConfig::default());
        ns.repo_cfg(egress);
        let cfg = json!({
            "box": { "provider": "namespace-devbox", "enforce_egress": true }
        });
        write_json(&ns.w.config_path(), cfg);
        ns.start(&ns.name("ns"));
        let warm = ns.verb("crabbox", "warmup").remove(0);
        let domains = warm
            .env
            .get("CRABBOX_NAMESPACE_EGRESS_DOMAINS")
            .expect("domains");
        assert!(domains.split(',').any(|d| d == "example.com"), "{domains}");
    }
);

parity!(
    acquire_failure_releases_attempted_handle_and_marks_box_failed,
    covers = ["A-28", "REQ-13"],
    tags = [],
    {
        let f = fx();
        let name = f.name("fail");
        let row = failed_acquire(&f, &name);
        assert_eq!(box_state(&row), BoxState::Failed);
        let slug = crabbox::slug(&f.w).expect("slug");
        let stop = f.verb("crabbox", "stop").remove(0);
        assert_eq!(stop.flag("--id"), Some(slug));
        assert_eq!(
            stop.env
                .get("CRABBOX_NAMESPACE_DELETE_ON_RELEASE")
                .map(String::as_str),
            Some("1")
        );
        assert_eq!(f.n("crabbox", "stop"), 1);
    }
);

parity!(
    acquire_failure_with_not_found_release_marks_box_gone,
    covers = ["A-28"],
    tags = [],
    {
        let f = fx();
        crabbox::not_found(&f.w);
        let row = failed_acquire(&f, &f.name("nf"));
        assert_eq!(box_state(&row), BoxState::Gone);
        assert!(row.boxed.unwrap().handle.is_none());
    }
);

parity!(
    acquire_failure_with_release_error_keeps_original_failure,
    covers = ["A-28"],
    tags = [],
    {
        let f = fx();
        crabbox::fail(&f.w, "stop");
        let row = failed_acquire(&f, &f.name("re"));
        assert_eq!(box_state(&row), BoxState::Failed);
        assert!(row.boxed.unwrap().handle.is_some());
    }
);

parity!(
    restart_after_acquire_failure_gets_a_new_box,
    covers = ["A-29"],
    tags = [],
    {
        let f = fx();
        let name = f.name("retry");
        f.w.prepend_rules(
            "crabbox",
            [Rule::on(&["warmup"])
                .exit(2)
                .stderr("quota exceeded for plan\n")
                .times(1)],
        );
        assert!(f.try_start(&name, &StartOpts::default()).class.is_err());
        assert_eq!(f.row(&name).state, TaskState::SetupFailed);
        let again = f.start(&name);
        assert_eq!(again.state, TaskState::Working);
        assert_eq!(box_state(&again), BoxState::Ready);
        assert_eq!(f.n("crabbox", "warmup"), 2);
        assert_eq!(f.rows(&name).len(), 1);
    }
);

parity!(
    failing_setup_keeps_task_working_with_box_and_agent,
    covers = ["A-03", "A-31", "A-34", "REQ-19"],
    tags = [],
    {
        let f = fx();
        f.repo_cfg(json!({ "setup": ["failing-step"] }));
        crabbox::fail_match(&f.w, "run", "failing-step", 7);
        let name = f.name("setup");
        let o = f.try_start(
            &name,
            &StartOpts {
                goal: Some("go".into()),
                ..Default::default()
            },
        );
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        assert!(!o.warnings.is_empty());
        let t = o.ok();
        assert_eq!(t.state, TaskState::Working);
        assert_eq!(box_state(t), BoxState::Ready);
        assert!(f.kinds(&name).contains(&EventKind::SetupFailed));
        assert_eq!(f.n("crabbox", "stop"), 0);
        assert_eq!(f.bg_calls().len(), 1);
        let run = f
            .verb("crabbox", "run")
            .into_iter()
            .find(|c| c.argv.iter().any(|a| a.contains("failing-step")))
            .expect("setup run");
        assert!(run.argv.iter().any(|a| a == "--no-hydrate"));
        assert!(!run.argv.iter().any(|a| a == "--no-sync"));
        assert_eq!(
            run.env
                .get("CRABBOX_SYNC_GIT_SEED_SOURCE")
                .map(String::as_str),
            Some("local")
        );
    }
);

parity!(
    rerun_after_setup_fix_keeps_box_and_clears_warnings,
    covers = ["A-31"],
    tags = [],
    {
        let f = fx();
        f.repo_cfg(json!({ "setup": ["failing-step"] }));
        crabbox::fail_match(&f.w, "run", "failing-step", 7);
        let name = f.name("fix");
        f.start(&name);
        f.repo_cfg(json!({ "setup": ["fixed-step"] }));
        let o = f.try_start(&name, &StartOpts::default());
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        assert!(o.warnings.is_empty(), "{:?}", o.warnings);
        assert_eq!(o.ok().state, TaskState::Working);
        assert_eq!(f.n("crabbox", "warmup"), 1);
        assert_eq!(f.n("crabbox", "stop"), 0);
    }
);

parity!(
    start_after_setup_failure_reruns_setup_only_once,
    covers = ["A-31", "B-54"],
    tags = ["new_only(SetupRetry)"],
    {
        let f = fx();
        f.repo_cfg(json!({ "setup": ["failing-step"] }));
        crabbox::fail_match(&f.w, "run", "failing-step", 7);
        let name = f.name("retry");
        f.start(&name);
        f.repo_cfg(json!({ "setup": ["fixed-step"] }));
        let fixed_runs = |f: &Fx| {
            f.verb("crabbox", "run")
                .into_iter()
                .filter(|c| c.argv.iter().any(|a| a.contains("fixed-step")))
                .count()
        };
        let again = f.start(&name);
        assert_eq!(again.state, TaskState::Working);
        assert_eq!(fixed_runs(&f), 1);
        assert_eq!(f.n("crabbox", "warmup"), 1);
        f.start(&name);
        assert_eq!(fixed_runs(&f), 1);
    }
);

parity!(
    provision_copies_and_symlinks_and_missing_source_does_not_stop_start,
    covers = ["A-32"],
    tags = [],
    {
        let f = fx();
        f.repo_cfg(
            json!({ "provision": ["secret.txt", { "path": "linked.txt", "mode": "symlink" }] }),
        );
        std::fs::write(f.repo.path.join("secret.txt"), "s3").unwrap();
        std::fs::write(f.repo.path.join("linked.txt"), "l").unwrap();
        let t = f.start(&f.name("prov"));
        let wt = std::path::PathBuf::from(t.worktree.unwrap());
        assert_eq!(
            std::fs::read_to_string(wt.join("secret.txt")).unwrap(),
            "s3"
        );
        assert!(!wt.join("secret.txt").is_symlink());
        assert_eq!(
            std::fs::read_link(wt.join("linked.txt")).unwrap(),
            f.repo.path.join("linked.txt")
        );

        f.repo_cfg(json!({ "provision": ["absent.txt"] }));
        let o = f.try_start(&f.name("miss"), &StartOpts::default());
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        assert_eq!(o.ok().state, TaskState::Working);
        assert!(!o.warnings.is_empty());
    }
);

parity!(
    dotfiles_dir_uploaded_and_installed_after_setup,
    covers = ["A-33"],
    tags = [],
    {
        let (f, home) = dotfiles_fx("box");
        let t = f.start(&f.name("dots"));
        assert_eq!(t.state, TaskState::Working);
        assert!(home.join(".moor-dotfiles/.bashrc").exists());
        assert!(home.join("dotfiles-ran").exists());
        let setup = run_with(&f, "setup-marker")[0].seq;
        let dots = run_with(&f, "MOOR_DOTFILES=1")[0].seq;
        assert!(setup < dots);
    }
);

parity!(
    dotfiles_failure_sets_setup_failed_keeps_box_and_retry_applies,
    covers = ["A-33"],
    tags = [],
    {
        let (f, home) = dotfiles_fx("box");
        f.w.prepend_rules(
            "crabbox",
            [Rule::on(&["run"])
                .contains("MOOR_DOTFILES=1")
                .exit(9)
                .stderr("install failed\n")
                .times(1)],
        );
        let name = f.name("dotfail");
        assert!(f.try_start(&name, &StartOpts::default()).class.is_err());
        let row = f.row(&name);
        assert_eq!(row.state, TaskState::SetupFailed);
        assert_eq!(box_state(&row), BoxState::Ready);
        assert_eq!(f.n("crabbox", "stop"), 0);
        let again = f.start(&name);
        assert_eq!(again.state, TaskState::Working);
        assert!(home.join("dotfiles-ran").exists());
        assert_eq!(f.n("crabbox", "warmup"), 1);
    }
);

parity!(
    dotfiles_skipped_for_host_agent_tasks,
    covers = ["A-33", "REQ-50"],
    tags = [],
    {
        let (f, home) = dotfiles_fx("host");
        assert_eq!(f.start(&f.name("hostdots")).state, TaskState::Working);
        assert!(run_with(&f, "setup-marker").len() == 1);
        assert!(run_with(&f, "MOOR_DOTFILES=1").is_empty());
        assert!(!home.join(".moor-dotfiles").exists());
        assert_never_called(&f.w.all_calls(), "ssh", &[]);
    }
);
