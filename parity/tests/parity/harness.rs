use parity::assert::assert_seq_global;
use parity::fake::Rule;
use parity::parity;

use crate::world;

fn wait_for(path: &std::path::Path) {
    for _ in 0..100 {
        if path.exists() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    panic!("timeout waiting for {}", path.display());
}

fn marked_sleep_pids(w: &parity::world::World, needle: &str) -> Vec<u32> {
    let marker = format!("PARITY_FAKES={}", w.fakes_dir().display());
    let mut out = Vec::new();
    for e in std::fs::read_dir("/proc").expect("proc").flatten() {
        let Some(pid) = e.file_name().to_str().and_then(|n| n.parse::<u32>().ok()) else {
            continue;
        };
        let cmd = std::fs::read(e.path().join("cmdline")).unwrap_or_default();
        let env = std::fs::read(e.path().join("environ")).unwrap_or_default();
        if String::from_utf8_lossy(&cmd).contains(needle)
            && env.split(|b| *b == 0).any(|x| x == marker.as_bytes())
        {
            out.push(pid);
        }
    }
    out
}

fn bg_leak_case(script: &str, needle: &str) {
    let w = world();
    w.add_rules("crabbox", [Rule::on(&["bg"]).exec(script)]);
    w.command(w.tool_path("crabbox"))
        .arg("bg")
        .output()
        .expect("spawn fake");
    let mut pids = Vec::new();
    for _ in 0..100 {
        pids = marked_sleep_pids(&w, needle);
        if !pids.is_empty() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(30));
    }
    assert!(!pids.is_empty(), "background child not found");
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || drop(w)));
    let err = r.expect_err("drop must panic on leak");
    let msg = err
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| err.downcast_ref::<&str>().map(|s| s.to_string()))
        .unwrap_or_default();
    assert!(msg.contains("leaked processes"), "unexpected panic: {msg}");
    for pid in pids {
        let alive =
            std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|s| !s.contains(") Z"));
        assert!(!alive, "pid {pid} survived teardown");
    }
}

parity!(
    leak_check_catches_background_child,
    covers = ["REQ-47"],
    tags = [],
    { bg_leak_case("sleep 31 >/dev/null 2>&1 &", "sleep") }
);

parity!(
    leak_check_catches_setsid_child,
    covers = ["REQ-47"],
    tags = [],
    { bg_leak_case("setsid sleep 30 >/dev/null 2>&1 < /dev/null &", "sleep") }
);

parity!(
    tmp_cred_guard_detects_new_entry,
    covers = ["REQ-47"],
    tags = [],
    {
        let mut w = world();
        let dir = w.root().join("credscratch");
        std::fs::create_dir_all(&dir).expect("create scratch");
        w.watch_creds_in(&dir);
        w.assert_no_new_creds_in(&dir);
        let path = dir.join(format!("moor-cred-parity-{}", w.task_name("guard")));
        std::fs::write(&path, b"x").expect("create entry");
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            w.assert_no_new_creds_in(&dir)
        }));
        std::fs::remove_file(&path).expect("remove entry");
        assert!(r.is_err(), "guard must panic on new entry");
        w.assert_no_new_creds_in(&dir);
    }
);

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
                &format!(
                    "trap '' TERM; touch {}/tmp/ready; while :; do sleep 0.1; done",
                    w.root().display()
                ),
            ])
            .spawn()
            .expect("spawn sh");
        w.track_pid(child.id());
        wait_for(&w.root().join("tmp/ready"));
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

parity!(
    fake_flags_wrapped_secret_in_env,
    covers = ["REQ-47"],
    tags = [],
    {
        let w = world();
        let secret = format!("sk-{}", w.task_name("wrapped"));
        w.register_secret(&secret);
        w.command(w.tool_path("crabbox"))
            .arg("list")
            .status()
            .expect("spawn fake");
        w.command(w.tool_path("crabbox"))
            .arg("list")
            .env("LEAKPROBE", format!("Bearer {secret}"))
            .status()
            .expect("spawn fake");
        let calls = w.calls("crabbox");
        assert_eq!(calls.len(), 2);
        assert!(!calls[0].env_contains_secret, "control call flagged");
        assert!(calls[1].env_contains_secret, "wrapped secret not flagged");
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            parity::assert::assert_secret_absent(&w, &secret, &[])
        }));
        let err = r.expect_err("wrapped secret must fail assert_secret_absent");
        let msg = err
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| err.downcast_ref::<&str>().map(|s| s.to_string()))
            .unwrap_or_default();
        assert!(msg.contains("wrapped"), "unexpected panic: {msg}");
    }
);

parity!(
    assert_secret_absent_requires_registration,
    covers = ["REQ-47"],
    tags = [],
    {
        let w = world();
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            parity::assert::assert_secret_absent(&w, "never-registered", &[])
        }));
        let err = r.expect_err("unregistered secret must panic");
        let msg = err
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| err.downcast_ref::<&str>().map(|s| s.to_string()))
            .unwrap_or_default();
        assert!(msg.contains("register_secret"), "unexpected panic: {msg}");
    }
);
