use std::net::TcpStream;

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::Duration;

use parity::adapter::ExitClass;

use parity::fake::Rule;
use parity::parity;

use serde_json::json;

use crate::moor;

use super::support::*;

parity!(
    proxy_supervisor_is_detached_and_runs_the_reverse_tunnel_in_the_worktree,
    covers = ["C-87", "C-88", "B-108", "B-111"],
    tags = [],
    {
        let f = fx();
        let o = f.start_named(&f.name, true);
        let task = o.ok().clone();
        let ep = moor().proxy_endpoint(&f.w, &f.name).expect("endpoint");
        let sup = f.pid(&f.name);
        assert_eq!(pgrp(sup), Some(sup), "own process group");
        for fd in 0..3 {
            let t = std::fs::read_link(format!("/proc/{sup}/fd/{fd}"));
            assert_eq!(t.ok(), Some(PathBuf::from("/dev/null")), "fd {fd}");
        }
        let args = cmdline(sup);
        let has = |flag: &str, val: &str| args.windows(2).any(|p| p[0] == flag && p[1] == val);
        assert!(args.iter().any(|a| a == "proxy"), "{args:?}");
        assert!(args.iter().any(|a| a == "serve"), "{args:?}");
        assert!(has("--box-port", &ep.box_port.to_string()), "{args:?}");
        let handle = task.boxed.clone().and_then(|b| b.handle).expect("handle");
        assert!(has("--handle", &handle), "{args:?}");
        let tunnel = &f.tunnels()[0];
        let box_name = handle.split_once('/').expect("provider/name").1;
        let want: Vec<String> = [
            "tunnel",
            "--reverse",
            &format!("{}:{}", ep.box_port, ep.port),
            "--id",
            box_name,
            "--json",
            "--provider",
            "namespace-devbox",
        ]
        .map(String::from)
        .to_vec();
        assert_eq!(tunnel.argv, want);
        assert_eq!(Some(tunnel.cwd.as_str()), task.worktree.as_deref());
        f.close(&f.name);
    }
);

parity!(
    proxy_close_stops_supervisor_tunnel_and_listener,
    covers = ["C-90"],
    tags = [],
    {
        let f = fx();
        let ep = f.up();
        let sup = f.pid(&f.name);
        let tunnel = f.tunnels()[0].pid;
        assert!(alive(sup) && alive(tunnel));
        f.close(&f.name);
        assert!(
            poll(10, || !alive(tunnel)),
            "tunnel {tunnel} outlived close"
        );
        assert!(poll(10, || !alive(sup)), "supervisor {sup} outlived close");
        let addr = format!("127.0.0.1:{}", ep.port).parse().expect("addr");
        let refused = TcpStream::connect_timeout(&addr, Duration::from_secs(1));
        assert!(refused.is_err(), "listener still open");
        assert!(poll(10, || !f.state_path(&f.name).exists()), "state stays");
    }
);

parity!(
    proxy_restarts_a_tunnel_that_exits,
    covers = ["C-89", "B-110", "REQ-38"],
    tags = [],
    {
        let f = fx();
        f.w.prepend_rules(
            "crabbox",
            [Rule::on(&["tunnel"])
                .times(1)
                .stdout("{\"box\":20000,\"host\":20000}\n")],
        );
        let ep = f.up();
        let sup = f.pid(&f.name);
        assert!(poll(15, || f.tunnels().len() >= 2), "no second tunnel");
        assert_eq!(f.pid(&f.name), sup, "same supervisor");
        assert_eq!(f.state(&f.name)["ready"], json!(true));
        assert_eq!(f.state(&f.name)["nonce"], json!(ep.nonce));
        f.close(&f.name);
    }
);

parity!(
    proxy_check_reports_a_dead_supervisor_without_a_box_request,
    covers = ["C-93", "C-94"],
    tags = [],
    {
        let f = fx();
        f.up();
        let sup = f.pid(&f.name);
        unsafe { libc::kill(-(sup as i32), libc::SIGKILL) };
        assert!(poll(10, || !alive(sup)));
        let ssh_before = f.w.calls("ssh").len();
        let o = moor().proxy_check(&f.w, &f.repo.path, &f.name);
        assert_eq!(o.class, ExitClass::Failed, "{}", o.stdout);
        let c = o.value.as_ref().expect("report on stdout");
        assert!(!c.supervisor_alive);
        assert_eq!(c.tunnel, "unknown");
        assert_eq!(c.box_status, None);
        assert!(c.box_port.is_some() && c.host_port.is_some());
        assert_eq!(f.w.calls("ssh").len(), ssh_before, "no Box request");
        let missing = moor().proxy_check(&f.w, &f.repo.path, "no-such-task");
        assert_eq!(missing.class, ExitClass::Failed);
        f.close_plain(&f.name);
    }
);

parity!(
    proxy_check_probes_through_the_tunnel_with_status_classes,
    covers = ["C-93", "C-94", "C-95", "C-96"],
    tags = [],
    {
        let f = fx();
        let ep = f.up();
        let dirs_before = cred_dirs(&f.w);
        let down = moor().proxy_check(&f.w, &f.repo.path, &f.name);
        assert_eq!(down.class, ExitClass::Failed);
        let dc = down.value.as_ref().expect("report on stdout");
        assert!(dc.supervisor_alive);
        assert_eq!((dc.tunnel.as_str(), dc.box_status), ("down", Some(0)));
        let (stop, handle) = relay(ep.box_port, ep.port);
        let up = moor().proxy_check(&f.w, &f.repo.path, &f.name);
        assert_eq!(up.class, ExitClass::Ok, "{}{}", up.stdout, up.stderr);
        let c = up.ok();
        assert_eq!((c.tunnel.as_str(), c.box_status), ("up", Some(403)));
        assert_eq!(
            (c.box_port, c.host_port),
            (Some(ep.box_port), Some(ep.port))
        );
        stop.store(true, Ordering::Relaxed);
        handle.join().expect("relay");
        assert_eq!(cred_dirs(&f.w), dirs_before, "probe dir removed");
        assert_nonce_host_clean(&f.w, &ep.nonce);
        f.close(&f.name);
    }
);

fn needs_backoff_clock() {
    assert!(
        moor().caps().contains(&"ProxyBackoffClock"),
        "adapter cannot shrink the supervisor backoff"
    );
}

parity!(
    proxy_that_never_becomes_ready_gives_up_after_five_attempts,
    covers = ["B-109"],
    tags = ["new_only(ProxyBackoffClock)"],
    {
        needs_backoff_clock();
        let f = fx();
        std::fs::write(
            f.w.root().join("proxy-backoff.json"),
            json!({ "ms": 20 }).to_string(),
        )
        .expect("write proxy-backoff.json");
        f.w.prepend_rules(
            "crabbox",
            [Rule::on(&["tunnel"]).stdout("not json\n").block()],
        );
        let _ = f.start_named(&f.name, true);
        let gave_up = poll(2, || !f.state_path(&f.name).exists());
        assert!(gave_up, "supervisor never gave up");
        assert_eq!(f.tunnels().len(), 5, "five unready attempts");
        let ran =
            f.w.calls("herdr")
                .into_iter()
                .filter(|c| c.argv.starts_with(&["pane".to_string(), "run".to_string()]))
                .count();
        assert_eq!(ran, 0, "agent pane started without a proxy");
        f.close_plain(&f.name);
    }
);

parity!(
    anthropic_route_has_no_path_or_method_allow_list,
    covers = ["C-69"],
    tags = [],
    {
        let f = fx();
        let ep = f.up();
        let cases = [
            ("GET", "/"),
            ("GET", "/v1/messages"),
            ("POST", "/v1/messages"),
            ("DELETE", "/v1/organizations/o1/api_keys/k1"),
            ("PUT", "/admin/anything"),
            ("PATCH", "/v1/oauth/token"),
            ("GET", "/api/oauth/profile"),
        ];
        expect_all(&ep, 502, &cases);
        let log = f.log(&f.name);
        for (m, p) in cases {
            assert!(
                log.contains(&format!("{m} anthropic {p} 502")),
                "{m} {p}: {log}"
            );
        }
        f.close(&f.name);
    }
);

parity!(
    hidden_proxy_serve_refuses_a_closed_task_and_a_missing_state,
    covers = ["C-91"],
    tags = [],
    {
        let f = fx();
        let started = f.start_named(&f.name, false);
        assert_eq!(started.class, ExitClass::Ok, "{}", started.stderr);
        let wt = started
            .value
            .and_then(|t| t.worktree)
            .expect("task worktree");
        let serve = |wt: &str| {
            moor().github_raw(
                &f.w,
                &f.repo.path,
                &[
                    "proxy",
                    "serve",
                    "--worktree",
                    wt,
                    "--handle",
                    "h",
                    "--box-port",
                    "1",
                ],
            )
        };
        let open = serve(&wt);
        assert_eq!(open.class, ExitClass::Failed, "{}", open.stderr);
        assert!(
            open.stderr
                .contains(&format!("no proxy state for task {}", f.name)),
            "{}",
            open.stderr
        );
        f.close_plain(&f.name);
        let closed = serve(&wt);
        assert_eq!(closed.class, ExitClass::Failed, "{}", closed.stderr);
        assert!(
            closed
                .stderr
                .contains(&format!("no open task for worktree {wt}")),
            "{}",
            closed.stderr
        );
        assert!(!f.state_path(&f.name).exists());
    }
);
