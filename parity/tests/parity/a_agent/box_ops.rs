use std::time::{Duration, Instant};

use parity::adapter::{BoxState, ExitClass, TaskState};
use parity::assert::assert_order;
use parity::fake::crabbox::{self, Preset};
use parity::fake::herdr;
use parity::parity;
use parity::world::hooks::HookState;

use super::support::*;

parity!(
    rerun_start_reacquires_a_gone_box,
    covers = ["B-75"],
    tags = [],
    {
        let fx = box_fx();
        fx.start(false);
        let first = fx.slug();
        crabbox::inventory(&fx.w, &["somebody-else"]);
        let gone = fx.row();
        assert_eq!(fx.box_state(&gone), BoxState::Gone);
        let again = fx.start(false);
        assert_eq!(fx.verb("warmup").len(), 2);
        assert_eq!(fx.box_state(&again), BoxState::Ready);
        assert_ne!(fx.slug(), first);
        assert_eq!(
            again
                .boxed
                .and_then(|b| b.handle)
                .as_deref()
                .map(|h| h.ends_with(&fx.slug())),
            Some(true)
        );
    }
);

parity!(
    waiting_task_refuses_run_cp_and_shell,
    covers = ["B-97"],
    tags = [],
    {
        let fx = box_fx_max(1);
        fx.start(false);
        fx.present();
        let queued = fx.w.task_name("queued");
        let t = fx.start_named(&queued, false);
        assert_eq!(t.state, TaskState::WaitingForBox);
        let dest = fx.w.root().join("never");
        assert!(fx
            .m
            .run(
                &fx.w,
                &fx.repo.path,
                &queued,
                &["true"],
                &Default::default()
            )
            .is_err());
        assert!(fx
            .m
            .cp(&fx.w, &fx.repo.path, &queued, "/tmp/x", &dest)
            .is_err());
        assert!(fx.m.shell(&fx.w, &fx.repo.path, &queued).is_err());
        assert!(fx.verb("cp").is_empty());
        assert!(fx.verb("connect").is_empty());
        assert!(fx
            .verb("run")
            .iter()
            .all(|c| !c.argv.iter().any(|a| a == "true")));
    }
);

parity!(
    repo_layer_box_provider_is_rejected,
    covers = ["B-85", "REQ-30"],
    tags = [],
    {
        let fx = box_fx();
        std::fs::write(
            fx.repo.path.join(".moor.yaml"),
            r#"{"box":{"provider":"sprites"}}"#,
        )
        .unwrap();
        let opts = parity::adapter::StartOpts::default();
        let o = fx.m.start(&fx.w, &fx.repo.path, &fx.name, &opts);
        assert!(o.is_err(), "{}", o.stdout);
        assert!(fx.verb("warmup").is_empty());
    }
);

parity!(
    namespace_start_warns_unless_egress_is_enforced,
    covers = ["B-88"],
    tags = [],
    {
        let fx = box_fx();
        let o =
            fx.m.start(&fx.w, &fx.repo.path, &fx.name, &Default::default());
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        assert!(!o.warnings.is_empty(), "namespace start must warn");
        let warm = &fx.verb("warmup")[0];
        assert!(!warm.env.contains_key("CRABBOX_NAMESPACE_EGRESS_DOMAINS"));

        let other = box_fx_on(Preset::LocalContainer);
        let o = other
            .m
            .start(&other.w, &other.repo.path, &other.name, &Default::default());
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        assert!(o.warnings.is_empty(), "{:?}", o.warnings);
    }
);

parity!(
    unknown_provider_takes_the_generic_path,
    covers = ["B-91"],
    tags = [],
    {
        let fx = box_fx_on(Preset::LocalContainer);
        user_edit(&fx.w, |d| d["box"]["provider"] = "acme-cloud".into());
        let task = fx.start(true);
        fx.present();
        assert_eq!(task.state, TaskState::Working);
        let slug = fx.slug();
        let w1 = fx.verb("warmup");
        assert_eq!(w1[0].flag("--provider").as_deref(), Some("acme-cloud"));
        let ran = fx.m.run(
            &fx.w,
            &fx.repo.path,
            &fx.name,
            &["true"],
            &Default::default(),
        );
        assert_eq!(ran.class, ExitClass::Ok, "{}", ran.stderr);
        let runs = fx.verb("run");
        assert_eq!(runs[0].flag("--provider").as_deref(), Some("acme-cloud"));
        assert_eq!(runs[0].flag("--id"), Some(slug.clone()));
        fx.w.hook_state(HookState::Idle, 0);
        assert_eq!(fx.row().state, TaskState::Idle);
        fx.m.age(&fx.w, &fx.name, 120);
        let t = fx.row();
        assert_eq!(t.state, TaskState::Idle);
        assert!(t.sleep_reason().is_some());
        assert!(fx.verb("pause").is_empty());
        let closed = fx.m.close(&fx.w, &fx.repo.path, &fx.name, false);
        assert_eq!(closed.class, ExitClass::Ok, "{}", closed.stderr);
        let stop = &fx.verb("stop")[0];
        assert_eq!(stop.flag("--provider").as_deref(), Some("acme-cloud"));
        assert_eq!(stop.flag("--id"), Some(slug));

        let bad = box_fx_on(Preset::LocalContainer);
        user_edit(&bad.w, |d| d["box"]["provider"] = "acme-cloud".into());
        crabbox::fail_quota(&bad.w);
        let o = bad
            .m
            .start(&bad.w, &bad.repo.path, &bad.name, &Default::default());
        assert_eq!(o.class, ExitClass::Failed);
    }
);

parity!(
    box_state_reads_go_over_the_crabbox_ssh_channel,
    covers = ["B-112", "B-113"],
    tags = [],
    {
        let fx = box_fx();
        fx.start(true);
        fx.present();
        fx.w.hook_state(HookState::Idle, 0);
        assert_eq!(states(&fx.wait_agent(5)), (Some("idle"), Some("hooks")));
        let ssh = fx.verb("ssh");
        assert!(!ssh.is_empty());
        assert_eq!(
            ssh[0].flag("--provider").as_deref(),
            Some("namespace-devbox")
        );
        assert_eq!(ssh[0].flag("--id"), Some(fx.slug()));
        let read =
            fx.w.calls("ssh")
                .into_iter()
                .rfind(|c| c.argv.last().is_some_and(|a| a.contains("agent-state")))
                .expect("hook read over ssh");
        assert_eq!(&read.argv[..3], ["-o", "BatchMode=yes", "box"]);
        let script = read.argv.last().unwrap();
        assert!(script.contains(&fx.w.hook_state_path().display().to_string()));
        assert!(
            script.contains("44"),
            "absent file maps to a distinct exit: {script}"
        );

        fx.w.clear_hook_state();
        herdr::pane_list(&fx.w, Some("working"));
        fx.m.age(&fx.w, &fx.name, 200);
        assert_eq!(states(&fx.pane_agent()), (Some("working"), Some("herdr")));

        crabbox::deny(&fx.w, "ssh", 1);
        let a = fx.pane_agent();
        assert_eq!(states(&a), (Some("working"), Some("herdr")));
    }
);

parity!(
    workdir_is_resolved_once_and_again_after_wake,
    covers = ["B-114"],
    tags = [],
    {
        let fx = box_fx();
        fx.start(true);
        fx.present();
        fx.w.hook_state(HookState::Idle, 0);
        let first = probes(&fx.w);
        assert_eq!(first, 1);
        fx.wait_agent(5);
        fx.pane_agent();
        fx.wait_agent(5);
        assert_eq!(probes(&fx.w), first, "reads reuse the resolved workdir");
        assert_eq!(fx.row().state, TaskState::Idle);
        fx.m.age(&fx.w, &fx.name, 120);
        assert_eq!(fx.row().state, TaskState::Asleep);
        let o = fx.m.steer(&fx.w, &fx.repo.path, &fx.name, "wake up");
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        fx.wait_agent(5);
        assert!(probes(&fx.w) > first, "wake drops the cached workdir");
    }
);

parity!(
    reads_stay_bounded_while_a_run_holds_the_box,
    covers = ["REQ-01", "REQ-08"],
    tags = [],
    {
        let fx = box_fx();
        fx.start(true);
        fx.present();
        let done = fx.m.run(
            &fx.w,
            &fx.repo.path,
            &fx.name,
            &["echo first-run"],
            &Default::default(),
        );
        assert_eq!(done.class, ExitClass::Ok, "{}", done.stderr);
        fx.w.prepend_rules(
            "crabbox",
            [parity::fake::Rule::on(&["run"])
                .contains("hold-run")
                .block()],
        );
        let mut run = fx.m.run_bg(&fx.w, &fx.repo.path, &fx.name, "hold-run");
        wait_for_call(&fx.w, "crabbox", "hold-run");
        let began = Instant::now();
        assert_eq!(fx.row().state, TaskState::Working);
        let p = fx.m.pane(&fx.w, &fx.repo.path, &fx.name);
        assert_eq!(p.class, ExitClass::Ok, "{}", p.stderr);
        let v = timed_out(&fx, 1);
        assert!(
            began.elapsed() < Duration::from_secs(8),
            "{:?}",
            began.elapsed()
        );
        let current = v.run.expect("in-flight run");
        assert!(current.to_string().contains("hold-run"), "{current}");
        assert_eq!(current["in_progress"], true, "{current}");
        run.kill().expect("kill run");
        run.wait().expect("reap run");
        let v = timed_out_or_done(&fx);
        let after = v.run.expect("interrupted run");
        assert!(after.to_string().contains("hold-run"), "{after}");
        assert_eq!(after["interrupted"], true, "{after}");
    }
);

parity!(
    dead_provisioning_refuses_run_and_restart,
    covers = ["B-76"],
    tags = [],
    {
        let fx = box_fx();
        fx.w.prepend_rules("crabbox", [parity::fake::Rule::on(&["warmup"]).block()]);
        let mut start =
            fx.m.start_detached(&fx.w, &fx.repo.path, &fx.name, &Default::default());
        wait_for_call(&fx.w, "crabbox", "warmup");
        start.kill().expect("kill start");
        start.wait().expect("reap start");
        fx.m.age(&fx.w, &fx.name, 300);
        let warmups = fx.verb("warmup").len();
        let again =
            fx.m.start(&fx.w, &fx.repo.path, &fx.name, &Default::default());
        assert!(again.is_err(), "{}", again.stdout);
        let run = fx.m.run(
            &fx.w,
            &fx.repo.path,
            &fx.name,
            &["true"],
            &Default::default(),
        );
        assert!(run.is_err());
        assert_eq!(fx.verb("warmup").len(), warmups);
    }
);

parity!(
    dotfiles_dir_is_uploaded_and_installed,
    covers = ["B-115"],
    tags = [],
    {
        let fx = box_fx();
        let dots = fx.w.root().join("dots");
        std::fs::create_dir_all(&dots).unwrap();
        std::fs::write(dots.join(".bashrc"), "# dots\n").unwrap();
        std::os::unix::fs::symlink(".bashrc", dots.join("linked")).unwrap();
        let install = "#!/bin/sh\ntouch \"$HOME/dotfiles-ran\"\n";
        std::fs::write(dots.join("install.sh"), install).unwrap();
        user_edit(&fx.w, |d| {
            d["dotfiles"] = serde_json::json!({ "dir": dots.display().to_string() })
        });
        let o =
            fx.m.start(&fx.w, &fx.repo.path, &fx.name, &Default::default());
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        let home = fx.w.box_root().join("home");
        assert!(
            home.join("dotfiles-ran").exists(),
            "install.sh ran in the Box"
        );
        let uploaded = std::fs::read_dir(&home)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .find(|p| p.join(".bashrc").exists())
            .expect("dotfiles directory in the Box home");
        let linked = std::fs::symlink_metadata(uploaded.join("linked")).unwrap();
        assert!(
            !linked.file_type().is_symlink(),
            "symlinks are dereferenced"
        );
        assert_order(
            &fx.w.all_calls(),
            &[("ssh", &["-o"]), ("crabbox", &["run"])],
        );
    }
);
