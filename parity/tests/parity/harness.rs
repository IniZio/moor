use parity::assert::assert_seq_global;
use parity::fake::Rule;
use parity::parity;

use crate::world;

fn run(w: &parity::world::World, tool: &str, args: &[&str]) -> String {
    let out = w
        .command(w.tool_path(tool))
        .args(args)
        .output()
        .expect("spawn fake");
    String::from_utf8(out.stdout).expect("utf8")
}

parity!(
    leak_check_fails_when_child_survives,
    covers = ["REQ-47"],
    tags = [],
    {
        let w = world();
        let mut child = w
            .command("sh")
            .args([
                "-c",
                "trap '' TERM; while :; do sleep 0.1; done",
                w.root().to_str().expect("utf8 root"),
            ])
            .spawn()
            .expect("spawn sh");
        w.track_pid(child.id());
        std::thread::sleep(std::time::Duration::from_millis(300));
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || drop(w)));
        let err = r.expect_err("drop must panic on leak");
        let msg = err
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| err.downcast_ref::<&str>().map(|s| s.to_string()))
            .unwrap_or_default();
        assert!(msg.contains("leaked processes"), "unexpected panic: {msg}");
        let status = child.wait().expect("child reaped");
        assert!(!status.success());
    }
);

parity!(
    leak_check_passes_when_clean,
    covers = ["REQ-47"],
    tags = [],
    {
        let mut w = world();
        w.add_rules("crabbox", [Rule::on(&["tunnel"]).block()]);
        let mut child = w
            .command(w.tool_path("crabbox"))
            .args(["tunnel", "--reverse"])
            .spawn()
            .expect("spawn fake");
        for _ in 0..100 {
            if w.calls("crabbox").len() == 1 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
        assert_eq!(w.calls("crabbox").len(), 1);
        w.teardown().expect("clean teardown");
        child.wait().expect("child reaped");
    }
);

parity!(
    fake_records_with_global_seq_and_rules,
    covers = [],
    tags = [],
    {
        let w = world();
        w.add_rules(
            "crabbox",
            [
                Rule::on(&["warmup"])
                    .times(1)
                    .stdout("first {n} {flag:--slug}"),
                Rule::on(&["warmup"]).stdout("later"),
                Rule::on(&["run"])
                    .exec("echo {box}/tmp/x")
                    .rewrite("/tmp/x", "/tmp/y"),
            ],
        );
        let out = w
            .command(w.tool_path("crabbox"))
            .args(["warmup", "--slug", "s1"])
            .env("MOOR_X", "1")
            .output()
            .expect("spawn");
        assert_eq!(String::from_utf8_lossy(&out.stdout), "first 1 s1");
        assert_eq!(run(&w, "crabbox", &["warmup", "--slug", "s2"]), "later");
        let dflt = w
            .command(w.tool_path("herdr"))
            .args(["pane", "list"])
            .output()
            .expect("spawn");
        assert_eq!(dflt.status.code(), Some(0));
        assert!(dflt.stdout.is_empty());
        let exec = run(&w, "crabbox", &["run", "--", "x"]);
        assert_eq!(exec.trim(), format!("{}/tmp/y", w.box_root().display()));

        let calls = w.calls("crabbox");
        assert_eq!(calls[0].argv, ["warmup", "--slug", "s1"]);
        assert_eq!(calls[0].cwd, w.root().to_str().expect("utf8"));
        assert_eq!(calls[0].env.get("MOOR_X").map(String::as_str), Some("1"));
        assert!(!calls[0].env.contains_key("PARITY_FAKES"));
        assert_eq!(w.calls("herdr").len(), 1);
        assert_seq_global(&w.all_calls());
    }
);
