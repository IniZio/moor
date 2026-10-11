use std::io::{Read, Write};
use std::net::TcpStream;

use std::time::Duration;

use parity::assert::assert_secret_absent;
use parity::parity;

use serde_json::json;

use super::support::*;
use super::upstream_support::*;

parity!(
    proxy_claude_route_swaps_auth_for_the_real_token,
    covers = ["C-68"],
    tags = ["new_only(ProxyUpstream)"],
    {
        needs_upstream();
        let f = fx();
        let up = upstream(echo_ok());
        use_upstream(&f, &up);
        let real = "real-claude-token-1";
        f.w.register_secret(real);
        let file = secret_file(&f, "claude", real);
        claude_user(&f, &file);
        let ep = f.up();
        let auth = format!("Bearer {}", ep.nonce);
        let r = call(
            ep.port,
            "POST",
            "/v1/messages?beta=1",
            &[
                ("Authorization", &auth),
                ("x-api-key", "placeholder"),
                ("anthropic-beta", "b1"),
                ("anthropic-version", "2023-06-01"),
                ("Content-Type", "application/json"),
            ],
            "{\"a\":1}",
        );
        assert_eq!(r.status(), Some(200), "{}", r.raw);
        let seen = up.seen.lock().unwrap().clone();
        let s = &seen[0];
        assert_eq!(
            (s.method.as_str(), s.target.as_str()),
            ("POST", "/v1/messages?beta=1")
        );
        assert_eq!(
            s.header("authorization"),
            Some("Bearer real-claude-token-1")
        );
        assert_eq!(s.header("x-api-key"), None);
        assert_eq!(s.header("anthropic-beta"), Some("b1"));
        assert_eq!(s.header("anthropic-version"), Some("2023-06-01"));
        assert_eq!(s.body, "{\"a\":1}");
        let by_path = call(
            ep.port,
            "GET",
            &format!("/t/{}/v1/models", ep.nonce),
            &[("Authorization", "Bearer placeholder")],
            "",
        );
        assert_eq!(by_path.status(), Some(200));
        std::fs::write(&file, "real-claude-token-2").expect("rotate token");
        let _ = via(&ep, "GET", "/v1/models");
        let seen = up.seen.lock().unwrap().clone();
        assert_eq!(seen[1].target, "/v1/models");
        assert_eq!(
            seen[2].header("authorization"),
            Some("Bearer real-claude-token-2")
        );
        for s in &seen {
            assert!(s.headers.iter().all(|(_, v)| !v.contains(&ep.nonce)));
        }
        assert_secret_absent(&f.w, real, &[]);
        f.close(&f.name);
    }
);

parity!(
    proxy_git_route_injects_the_minted_token_and_caches_it,
    covers = ["C-71", "C-72"],
    tags = ["new_only(ProxyUpstream)"],
    {
        needs_upstream();
        let f = fx_repo();
        let up = upstream(echo_ok());
        use_upstream(&f, &up);
        let (cmd, counter) = token_cmd(&f, "ghs_minted_1");
        user_with_token_cmd(&f, &cmd);
        let ep = f.up();
        let auth = format!("Bearer {}", ep.nonce);
        let target = format!(
            "/t/{}/github/acme/widget/info/refs?service=git-upload-pack",
            ep.nonce
        );
        for _ in 0..2 {
            let r = call(ep.port, "GET", &target, &[("Authorization", &auth)], "");
            assert_eq!(r.status(), Some(200), "{}", r.raw);
        }
        let seen = up.seen.lock().unwrap().clone();
        let basic = "Basic eC1hY2Nlc3MtdG9rZW46Z2hzX21pbnRlZF8x";
        assert_eq!(seen[0].header("authorization"), Some(basic));
        assert_eq!(
            seen[0].target,
            "/acme/widget/info/refs?service=git-upload-pack"
        );
        let mints = std::fs::read_to_string(&counter)
            .unwrap_or_default()
            .lines()
            .count();
        assert_eq!(mints, 1, "token cached between requests");
        assert_secret_absent(&f.w, "ghs_minted_1", &[]);
        f.close(&f.name);
    }
);

parity!(
    proxy_rest_route_forwards_with_token_and_default_headers,
    covers = ["C-73", "C-75"],
    tags = ["new_only(ProxyUpstream)"],
    {
        needs_upstream();
        let f = fx_repo();
        let up = upstream(echo_ok());
        use_upstream(&f, &up);
        let (cmd, _) = token_cmd(&f, "ghs_rest_1");
        user_with_token_cmd(&f, &cmd);
        let ep = f.up();
        let r = via(&ep, "GET", "/github-api/repos/acme/widget/pulls?state=open");
        assert_eq!(r.status(), Some(200), "{}", r.raw);
        let kept = call(
            ep.port,
            "GET",
            &format!("/t/{}/github-api/repos/acme/widget/pulls/3", ep.nonce),
            &[("Accept", "text/plain"), ("User-Agent", "agent/1")],
            "",
        );
        assert_eq!(kept.status(), Some(200));
        let seen = up.seen.lock().unwrap().clone();
        assert_eq!(seen[0].target, "/repos/acme/widget/pulls?state=open");
        assert_eq!(seen[0].header("authorization"), Some("token ghs_rest_1"));
        assert_eq!(
            seen[0].header("accept"),
            Some("application/vnd.github+json")
        );
        assert_eq!(seen[0].header("user-agent"), Some("moor-proxy"));
        assert_eq!(seen[1].header("accept"), Some("text/plain"));
        assert_eq!(seen[1].header("user-agent"), Some("agent/1"));
        f.close(&f.name);
    }
);

parity!(
    proxy_comment_gate_requires_a_pull_request,
    covers = ["C-74"],
    tags = ["new_only(ProxyUpstream)"],
    {
        needs_upstream();
        let f = fx_repo();
        let up = upstream(Box::new(|req, out| {
            let status = if req.target.ends_with("/pulls/7") {
                404
            } else {
                200
            };
            reply(out, status, "{}");
        }));
        use_upstream(&f, &up);
        let (cmd, _) = token_cmd(&f, "ghs_gate_1");
        user_with_token_cmd(&f, &cmd);
        let ep = f.up();
        let plain = via(
            &ep,
            "POST",
            "/github-api/repos/acme/widget/issues/7/comments",
        );
        assert_eq!(plain.status(), Some(403), "{}", plain.raw);
        assert_eq!(
            up.seen.lock().unwrap().len(),
            1,
            "only the pull lookup was sent"
        );
        let pull = via(
            &ep,
            "POST",
            "/github-api/repos/acme/widget/issues/8/comments",
        );
        assert_eq!(pull.status(), Some(200), "{}", pull.raw);
        let seen = up.seen.lock().unwrap().clone();
        let tail: Vec<&str> = seen[1..].iter().map(|s| s.target.as_str()).collect();
        assert_eq!(
            tail,
            [
                "/repos/acme/widget/pulls/8",
                "/repos/acme/widget/issues/8/comments"
            ]
        );
        f.close(&f.name);
    }
);

parity!(
    proxy_rewrites_the_origin_name_to_the_canonical_repo,
    covers = ["C-76", "REQ-38"],
    tags = ["new_only(ProxyUpstream)"],
    {
        needs_upstream();
        let f = fx_repo();
        let up = upstream(Box::new(|req, out| {
            if req.target == "/repos/acme/widget" {
                let doc = "{\"name\":\"gadget\",\"id\":5,\"owner\":{\"login\":\"acme\",\"id\":9}}";
                reply(out, 200, doc);
            } else {
                reply(out, 200, "{}");
            }
        }));
        use_upstream(&f, &up);
        let (cmd, _) = token_cmd(&f, "ghs_canon_1");
        user_with_token_cmd(&f, &cmd);
        let ep = f.up();
        let r = via(&ep, "GET", "/github-api/repos/acme/widget/pulls");
        assert_eq!(r.status(), Some(200), "{}", r.raw);
        let seen = up.seen.lock().unwrap().clone();
        assert_eq!(
            seen.last().map(|s| s.target.as_str()),
            Some("/repos/acme/gadget/pulls")
        );
        assert_eq!(f.state(&f.name)["canonical"], json!("acme/gadget"));
        f.close(&f.name);
    }
);

parity!(
    proxy_relays_streams_and_does_not_follow_redirects,
    covers = ["C-77", "C-78"],
    tags = ["new_only(ProxyUpstream)"],
    {
        needs_upstream();
        let f = fx();
        let up = upstream(Box::new(|req, out| {
            if req.target.starts_with("/redirect") {
                let head = "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/x\r\nContent-Length: 0\r\n\r\n";
                let _ = out.write_all(head.as_bytes());
                return;
            }
            let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n";
            let _ = out.write_all(head.as_bytes());
            for part in ["data: one\n\n", "data: two\n\n"] {
                let _ = write!(out, "{:x}\r\n{part}\r\n", part.len());
                let _ = out.flush();
                std::thread::sleep(Duration::from_millis(300));
            }
            let _ = out.write_all(b"0\r\n\r\n");
        }));
        use_upstream(&f, &up);
        let file = secret_file(&f, "claude", "real-claude-token-3");
        claude_user(&f, &file);
        let ep = f.up();
        let auth = format!("Bearer {}", ep.nonce);
        let mut s = TcpStream::connect(("127.0.0.1", ep.port)).expect("connect");
        let req = format!(
            "GET /v1/stream HTTP/1.1\r\nHost: x\r\nAuthorization: {auth}\r\nConnection: close\r\n\r\n"
        );
        s.write_all(req.as_bytes()).expect("send");
        s.set_read_timeout(Some(Duration::from_secs(5))).ok();
        let mut first = [0u8; 4096];
        let n = s.read(&mut first).expect("first read");
        let early = String::from_utf8_lossy(&first[..n]).into_owned();
        assert!(early.contains("data: one"), "first event delayed: {early}");
        assert!(
            !early.contains("data: two"),
            "events were buffered together"
        );
        let mut rest = String::new();
        let _ = s.read_to_string(&mut rest);
        assert!(rest.contains("data: two"), "{rest}");
        let moved = call(ep.port, "GET", "/redirect", &[("Authorization", &auth)], "");
        assert_eq!(moved.status(), Some(302), "{}", moved.raw);
        f.close(&f.name);
    }
);

parity!(
    proxy_config_error_after_start_is_not_confused_with_missing_token,
    covers = [],
    tags = ["new_only(ProxyUpstream)"],
    {
        needs_upstream();
        let f = fx();
        let up = upstream(echo_ok());
        use_upstream(&f, &up);
        let ep = f.up();
        let missing = via(&ep, "GET", "/v1/models");
        std::fs::write(f.w.config_path(), "box: [unclosed\n  : :").expect("break config");
        let broken = via(&ep, "GET", "/v1/models");
        assert_eq!(missing.status(), Some(502));
        assert_eq!(broken.status(), Some(502));
        assert_ne!(
            missing.body(),
            broken.body(),
            "a config error must read differently from 'token not configured'"
        );
        f.close(&f.name);
    }
);
