use std::net::{Shutdown, TcpStream};
use std::os::unix::fs::PermissionsExt;

use parity::adapter::ExitClass;

use parity::fake::herdr;
use parity::parity;

use serde_json::json;

use crate::moor;

use super::support::*;

parity!(
    proxy_listener_is_loopback_only_and_is_the_tunnel_host_port,
    covers = ["C-64", "C-88"],
    tags = [],
    {
        let f = fx();
        let ep = f.up();
        assert_eq!(listening_addrs(ep.port), ["0100007F"], "loopback only");
        let t = f.tunnels();
        assert_eq!(t.len(), 1, "one tunnel");
        assert_eq!(t[0].argv[2], format!("{}:{}", ep.box_port, ep.port));
        f.close(&f.name);
    }
);

parity!(
    proxy_requires_the_task_nonce_else_401,
    covers = ["C-65", "REQ-38"],
    tags = [],
    {
        let f = fx();
        let ep = f.up();
        let last = if ep.nonce.ends_with('0') { '1' } else { '0' };
        let near = format!("{}{last}", &ep.nonce[..ep.nonce.len() - 1]);
        let basic = format!("Basic {}", ep.nonce);
        let bad = format!("Bearer {near}");
        let denied: Vec<(&str, Vec<(&str, &str)>)> = vec![
            ("/v1/messages", vec![]),
            ("/v1/messages", vec![("Authorization", &bad)]),
            ("/v1/messages", vec![("Authorization", "Bearer ")]),
            ("/v1/messages", vec![("Authorization", &basic)]),
            ("/v1/messages", vec![("x-api-key", &ep.nonce)]),
            ("/t//v1/messages", vec![]),
            ("/t/x/v1/messages", vec![]),
        ];
        for (target, headers) in &denied {
            let r = call(ep.port, "GET", target, headers, "");
            assert_eq!(r.status(), Some(401), "{target} {headers:?}: {}", r.raw);
            assert_eq!(r.body(), "unauthorized\n");
            assert!(r.head_has("connection: close"));
        }
        let wrong_path = call(ep.port, "GET", &format!("/t/{near}/v1/x"), &[], "");
        assert_eq!(wrong_path.status(), Some(401));
        let by_header = bearer(&ep, "GET", "/v1/models");
        assert_eq!(by_header.status(), Some(502), "{}", by_header.raw);
        let by_path = via(&ep, "GET", "/v1/models");
        assert_eq!(by_path.status(), Some(502), "{}", by_path.raw);
        let bare = call(ep.port, "GET", &format!("/t/{}", ep.nonce), &[], "");
        assert_eq!(bare.status(), Some(502), "bare /t/<nonce> maps to /");
        f.close(&f.name);
    }
);

parity!(
    proxy_routes_by_prefix_with_a_separator,
    covers = ["C-66"],
    tags = [],
    {
        let f = fx();
        let ep = f.up();
        expect_all(
            &ep,
            502,
            &[
                ("GET", "/github-apix/foo"),
                ("GET", "/githubx/foo"),
                ("GET", "/v1/messages"),
            ],
        );
        expect_all(
            &ep,
            403,
            &[
                ("GET", "/github/foo"),
                ("GET", "/github?x=1"),
                ("GET", "/github-api/foo"),
                ("GET", "/github-api?x=1"),
                ("GET", "/github-api"),
            ],
        );
        let log = f.log(&f.name);
        assert!(log.contains("GET anthropic /githubx/foo 502"), "{log}");
        assert!(log.contains("GET github /foo 403"), "{log}");
        assert!(log.contains("GET github-api /foo 403"), "{log}");
        f.close(&f.name);
    }
);

parity!(
    proxy_rejects_a_target_that_does_not_start_with_slash,
    covers = ["C-67"],
    tags = [],
    {
        let f = fx();
        let ep = f.up();
        let r = bearer(&ep, "GET", "http://example.invalid/v1/messages");
        assert_eq!(r.status(), Some(400), "{}", r.raw);
        assert_eq!(r.body(), "bad request target\n");
        f.close(&f.name);
    }
);

parity!(
    proxy_claude_route_fails_502_when_the_token_is_unavailable,
    covers = ["C-68"],
    tags = [],
    {
        let f = fx();
        let ep = f.up();
        let none = via(&ep, "POST", "/v1/messages");
        assert_eq!(none.status(), Some(502));
        assert_eq!(none.body(), "credential unavailable\n");
        let missing = f.w.root().join("secrets/missing-claude");
        f.user(user_doc(json!({
            "credentials": { "CLAUDE_CODE_OAUTH_TOKEN": { "file": missing } }
        })));
        let unreadable = via(&ep, "POST", "/v1/messages");
        assert_eq!(unreadable.status(), Some(502));
        assert_eq!(unreadable.body(), none.body());
        f.close(&f.name);
    }
);

parity!(
    proxy_github_routes_deny_everything_without_a_task_repo,
    covers = ["C-70", "C-73", "REQ-38"],
    tags = [],
    {
        let f = fx();
        let ep = f.up();
        expect_all(
            &ep,
            403,
            &[
                ("GET", "/github/acme/widget/info/refs"),
                ("POST", "/github/acme/widget/git-upload-pack"),
                ("GET", "/github-api/repos/acme/widget/pulls"),
                ("POST", "/github-api/repos/acme/widget/pulls"),
            ],
        );
        f.close(&f.name);
    }
);

parity!(
    proxy_log_records_requests_without_secrets,
    covers = ["C-79"],
    tags = [],
    {
        let f = fx();
        let ep = f.up();
        let _ = call(ep.port, "GET", "/v1/x?q=querysecret", &[], "");
        let _ = via(&ep, "GET", "/github/other/repo/info/refs?k=querysecret");
        let _ = via(&ep, "GET", "/v1/models?k=querysecret");
        let log = f.log(&f.name);
        let lines: Vec<&str> = log.lines().collect();
        assert_eq!(
            lines[0],
            format!("proxy listening on 127.0.0.1:{}", ep.port)
        );
        assert!(lines.contains(&"GET none - 401"), "{log}");
        assert!(
            lines.contains(&"GET github /other/repo/info/refs 403"),
            "{log}"
        );
        assert!(lines.contains(&"GET anthropic /v1/models 502"), "{log}");
        assert!(!log.contains(&ep.nonce) && !log.contains("querysecret"));
        let dir = f.w.state_dir().join("proxy");
        assert_eq!(mode(&dir.join(format!("{}.log", f.name))), 0o600);
        assert_nonce_host_clean(&f.w, &ep.nonce);
        f.close(&f.name);
    }
);

parity!(
    proxy_tolerates_a_bare_connect_and_close,
    covers = ["C-80"],
    tags = [],
    {
        let f = fx();
        let ep = f.up();
        let before = f.log(&f.name).lines().count();
        let s = TcpStream::connect(("127.0.0.1", ep.port)).expect("connect");
        s.shutdown(Shutdown::Both).ok();
        drop(s);
        let r = call(ep.port, "GET", "/v1/models", &[], "");
        assert_eq!(r.status(), Some(401));
        let log = f.log(&f.name);
        assert_eq!(
            log.lines().count(),
            before + 1,
            "probe logged nothing: {log}"
        );
        assert!(alive(f.pid(&f.name)));
        f.close(&f.name);
    }
);

parity!(
    proxy_transport_limits_and_expect_continue,
    covers = ["C-77"],
    tags = [],
    {
        let f = fx();
        let ep = f.up();
        let mut many: Vec<(String, String)> =
            (0..101).map(|i| (format!("X-H{i}"), "v".into())).collect();
        many.push(("Authorization".into(), format!("Bearer {}", ep.nonce)));
        let refs: Vec<(&str, &str)> = many.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        let crowded = call(ep.port, "GET", "/v1/x", &refs, "");
        assert_eq!(crowded.status(), None, "more than 100 headers is dropped");
        let auth = format!("Bearer {}", ep.nonce);
        let cont = call(
            ep.port,
            "POST",
            "/v1/messages",
            &[("Authorization", &auth), ("Expect", "100-continue")],
            "{}",
        );
        assert_eq!(cont.statuses, [100, 502], "{}", cont.raw);
        f.close(&f.name);
    }
);

parity!(
    proxy_state_file_has_the_documented_shape_and_modes,
    covers = ["C-86"],
    tags = [],
    {
        let f = fx();
        let o = f.start_named(&f.name, true);
        let handle = o.ok().boxed.clone().and_then(|b| b.handle).expect("handle");
        let dir = f.w.state_dir().join("proxy");
        assert_eq!(mode(&dir), 0o700);
        assert_eq!(mode(&f.state_path(&f.name)), 0o600);
        let st = f.state(&f.name);
        assert_eq!(st["ready"], json!(true));
        assert_eq!(st["handle"], json!(handle));
        for key in ["nonce", "box_port", "pid", "host_port", "canonical"] {
            assert!(st.get(key).is_some(), "state lacks {key}: {st}");
        }
        assert!(st["host_port"].as_u64().unwrap_or(0) > 0);
        f.close(&f.name);
        assert!(poll(10, || !f.state_path(&f.name).exists()), "state stays");
    }
);

parity!(
    proxy_nonce_is_64_hex_fresh_per_supervisor_and_never_in_argv,
    covers = ["C-84"],
    tags = [],
    {
        let f = fx();
        let first = f.up();
        let other = f.w.task_name("px2");
        let second = f.up_named(&other);
        for ep in [&first, &second] {
            assert_eq!(ep.nonce.len(), 64);
            assert!(ep
                .nonce
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
        }
        assert_ne!(first.nonce, second.nonce);
        let sup = f.pid(&f.name);
        assert!(cmdline(sup).iter().all(|a| !a.contains(&first.nonce)));
        let checked = moor().proxy_check(&f.w, &f.repo.path, &f.name);
        assert!(!checked.stdout.contains(&first.nonce));
        f.close(&f.name);
        f.close(&other);
        assert_nonce_host_clean(&f.w, &first.nonce);
        assert_nonce_host_clean(&f.w, &second.nonce);
    }
);

parity!(
    proxy_ignores_a_foreign_pid_in_stale_state_and_keeps_its_port,
    covers = ["C-83"],
    tags = [],
    {
        let f = fx();
        let dir = f.w.state_dir().join("proxy");
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).expect("chmod");
        let stale = "a".repeat(64);
        let doc = json!({
            "nonce": stale, "box_port": 31337, "handle": "stale/handle", "pid": 1,
            "ready": true, "host_port": 1, "canonical": null
        });
        let path = f.state_path(&f.name);
        std::fs::write(&path, doc.to_string()).expect("write stale state");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).expect("chmod");
        let ep = f.up();
        assert_ne!(ep.nonce, stale, "a foreign pid is never reused");
        assert_eq!(ep.box_port, 31337, "a free recorded port is kept");
        assert_ne!(f.pid(&f.name), 1);
        assert_eq!(f.tunnels().len(), 1);
        f.close(&f.name);
    }
);

parity!(
    proxy_box_ports_follow_the_task_name_hash_and_probe_past_taken,
    covers = ["C-85"],
    tags = [],
    {
        let f = fx();
        let other = f.w.task_name("px2");
        let a = f.up();
        let b = f.up_named(&other);
        let span = |n: &str| 20000 + (fnv1a(n) % 20000) as u16;
        assert_ne!(a.box_port, b.box_port);
        assert_eq!(a.box_port, span(&f.name), "first task takes its hash port");
        let want = span(&other);
        let probed = if want == a.box_port { want + 1 } else { want };
        assert_eq!(b.box_port, probed);
        assert!(alive(f.pid(&f.name)) && alive(f.pid(&other)));
        f.close(&f.name);
        f.close(&other);
    }
);

parity!(
    proxy_starts_at_first_box_agent_launch_and_relaunch_reuses_it,
    covers = ["C-82", "C-83"],
    tags = [],
    {
        let f = fx();
        let idle = f.start_named(&f.name, false);
        assert_eq!(idle.class, ExitClass::Ok, "{}", idle.stderr);
        assert!(
            !f.w.state_dir().join("proxy").exists(),
            "no proxy without a goal"
        );
        assert!(f.tunnels().is_empty());
        let other = f.w.task_name("px2");
        let ep = f.up_named(&other);
        let sup = f.pid(&other);
        assert_eq!(f.tunnels().len(), 1);
        herdr::pane_gone(&f.w);
        let again = f.start_named(&other, true);
        assert_eq!(again.class, ExitClass::Ok, "{}", again.stderr);
        assert_eq!(f.tunnels().len(), 1, "relaunch reuses the tunnel");
        assert_eq!(f.pid(&other), sup);
        assert_eq!(f.state(&other)["nonce"], json!(ep.nonce));
        f.close(&other);
        f.close_plain(&f.name);
    }
);
