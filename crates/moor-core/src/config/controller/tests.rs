//! Controller config parse tables.
#![allow(clippy::panic, reason = "test assertions")]

use super::{parse_controller, ControllerConfig};

fn err(text: &str) -> crate::config::ConfigError {
    match parse_controller("c.yaml", text) {
        Ok(c) => panic!("accepted {text}: {c:?}"),
        Err(e) => e,
    }
}

#[test]
fn empty_documents_are_default() {
    for text in ["", "  \n", "null", "{}"] {
        let c = parse_controller("c.yaml", text).unwrap_or_else(|e| panic!("{text}: {e}"));
        assert_eq!(c, ControllerConfig::default(), "{text}");
    }
}

#[test]
fn both_keys_parse() {
    let c = parse_controller("c.yaml", "socket: ~/run/c.sock\nstate_dir: /var/moor\n")
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(c.socket.as_deref(), Some("~/run/c.sock"));
    assert_eq!(c.state_dir.as_deref(), Some("/var/moor"));
}

#[test]
fn unknown_key_names_key_and_file() {
    let e = err("socket: /a\nbogus: 1\n");
    assert_eq!(e.file, "c.yaml");
    assert!(e.to_string().contains("bogus"), "{e}");
    assert!(e.to_string().contains("c.yaml"), "{e}");
}

#[test]
fn mistyped_value_names_key() {
    let e = err("socket: [a, b]\n");
    assert_eq!(e.path, "socket");
    assert_eq!(e.file, "c.yaml");
}

#[test]
fn empty_string_is_rejected() {
    for key in ["socket", "state_dir"] {
        let e = err(&format!("{key}: \"\"\n"));
        assert_eq!(e.path, key);
        assert_eq!(e.file, "c.yaml");
    }
}

#[test]
fn settle_interval_parses_and_defaults_to_five_seconds() {
    let c = parse_controller("c.yaml", "settle_every_secs: 30\n").unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(c.settle_every(), std::time::Duration::from_secs(30));
    assert_eq!(
        ControllerConfig::default().settle_every(),
        std::time::Duration::from_secs(5)
    );
}

#[test]
fn drain_timeout_parses_and_defaults_to_thirty_seconds() {
    let c = parse_controller("c.yaml", "drain_timeout_secs: 0\n").unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(c.drain_timeout(), std::time::Duration::ZERO);
    assert_eq!(
        ControllerConfig::default().drain_timeout(),
        std::time::Duration::from_secs(30)
    );
}

#[test]
fn ledger_tls_parses_its_address_cert_key_and_client_ca() {
    let c = parse_controller(
        "c.yaml",
        "ledger_tls:\n  listen: 0.0.0.0:7443\n  cert: /tls/tls.crt\n  key: /tls/tls.key\n  client_ca: /tls/ca.pem\n",
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let tls = c.ledger_tls.unwrap_or_else(|| panic!("no ledger_tls"));
    assert_eq!(tls.listen.to_string(), "0.0.0.0:7443");
    assert_eq!(tls.cert, std::path::Path::new("/tls/tls.crt"));
    assert_eq!(tls.key, std::path::Path::new("/tls/tls.key"));
    assert_eq!(tls.client_ca, std::path::Path::new("/tls/ca.pem"));
    assert_eq!(ControllerConfig::default().ledger_tls, None);
}

#[test]
fn removed_agent_places_key_is_unknown() {
    let e = err("agent_places:\n  users: [ann@example.com]\n");
    assert_eq!(e.path, "agent_places");
    assert!(e.reason.contains("unknown field"), "{e}");
}

#[test]
fn github_app_parses_its_id_and_key_files() {
    let c = parse_controller(
        "c.yaml",
        "github_app:\n  id: 123456\n  private_key_file: /run/app/key.pem\n  webhook_secret_file: /run/app/secret\n",
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let app = c.github_app.unwrap_or_else(|| panic!("no github_app"));
    assert_eq!(app.id, 123_456);
    assert_eq!(
        app.private_key_file,
        std::path::Path::new("/run/app/key.pem")
    );
    assert_eq!(
        app.webhook_secret_file.as_deref(),
        Some(std::path::Path::new("/run/app/secret"))
    );
    assert_eq!(ControllerConfig::default().github_app, None);
}

#[test]
fn github_app_needs_an_id_and_a_key_file_but_not_a_webhook_secret() {
    let c = parse_controller(
        "c.yaml",
        "github_app:\n  id: 1\n  private_key_file: /k.pem\n",
    )
    .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(c.github_app.and_then(|a| a.webhook_secret_file), None);
    for text in [
        "github_app:\n  private_key_file: /k.pem\n",
        "github_app:\n  id: 1\n",
        "github_app:\n  id: 1\n  private_key_file: /k.pem\n  slug: x\n",
    ] {
        assert!(err(text).to_string().contains("github_app"), "{text}");
    }
}

#[test]
fn ledger_tls_needs_every_key_and_a_socket_address() {
    for (text, path) in [
        (
            "ledger_tls:\n  listen: 0.0.0.0:7443\n  cert: /c\n  client_ca: /a\n",
            "ledger_tls",
        ),
        (
            "ledger_tls:\n  listen: 0.0.0.0:7443\n  cert: /c\n  key: /k\n",
            "ledger_tls",
        ),
        (
            "ledger_tls:\n  listen: ledger\n  cert: /c\n  key: /k\n  client_ca: /a\n",
            "ledger_tls.listen",
        ),
        (
            "ledger_tls:\n  listen: 0.0.0.0:7443\n  cert: /c\n  key: /k\n  client_ca: /a\n  ca: /a\n",
            "ledger_tls",
        ),
    ] {
        let e = err(text);
        assert!(e.path.starts_with(path), "{text}: {e}");
    }
}

#[test]
fn zero_settle_interval_names_key() {
    let e = err("settle_every_secs: 0\n");
    assert_eq!(e.path, "settle_every_secs");
    assert_eq!(e.file, "c.yaml");
}

#[test]
fn removed_proof_cap_key_is_unknown() {
    let e = err("proof_max_bytes: 1024\n");
    assert_eq!(e.path, "proof_max_bytes");
    assert!(e.reason.contains("unknown field"), "{e}");
}

#[test]
fn a_repos_key_is_an_unknown_key() {
    let e = err("repos:\n  app:\n    url: https://github.com/o/app.git\n");
    assert!(e.to_string().contains("unknown field `repos`"), "{e}");
}

#[test]
fn fronts_parse_by_namespace() {
    let c = parse_controller("c.yaml", "fronts:\n  herdr: {}\n  web-2: {}\n")
        .unwrap_or_else(|e| panic!("{e}"));
    let names: Vec<&str> = c.fronts.keys().map(String::as_str).collect();
    assert_eq!(names, ["herdr", "web-2"]);
}

#[test]
fn a_reserved_or_malformed_front_namespace_is_refused() {
    for ns in ["local", "Herdr", "2x", "a_b"] {
        let e = err(&format!("fronts:\n  {ns}: {{}}\n"));
        assert_eq!(e.path, format!("fronts.{ns}"), "{e}");
        assert!(e.to_string().contains("not be `local`"), "{e}");
    }
    let e = err("fronts:\n  herdr: {bogus: 1}\n");
    assert!(e.to_string().contains("bogus"), "{e}");
}

#[test]
fn a_front_command_is_an_argv() {
    let c = parse_controller(
        "c.yaml",
        "fronts:\n  web:\n    command: [moor-front, --port, '8080']\n  herdr: {}\n",
    )
    .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        c.fronts.get("web").and_then(|f| f.command.as_deref()),
        Some(
            ["moor-front", "--port", "8080"]
                .map(String::from)
                .as_slice()
        )
    );
    assert_eq!(c.fronts.get("herdr").map(|f| f.command.clone()), Some(None));
}

#[test]
fn an_empty_front_command_is_refused() {
    let e = err("fronts:\n  web:\n    command: []\n");
    assert_eq!(e.path, "fronts.web.command", "{e}");
    assert!(e.to_string().contains("must name a program"), "{e}");
}

#[test]
fn a_front_state_dir_is_an_absolute_path() {
    let c = parse_controller("c.yaml", "fronts:\n  web:\n    state_dir: /srv/web\n")
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        c.fronts.get("web").and_then(|f| f.state_dir.as_deref()),
        Some(std::path::Path::new("/srv/web"))
    );
    let e = err("fronts:\n  web:\n    state_dir: srv/web\n");
    assert_eq!(e.path, "fronts.web.state_dir", "{e}");
    assert!(e.to_string().contains("must be an absolute path"), "{e}");
}

#[test]
fn a_front_read_only_list_holds_absolute_paths() {
    let c = parse_controller(
        "c.yaml",
        "fronts:\n  web:\n    read_only: [/etc/moor-front]\n",
    )
    .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        c.fronts.get("web").map(|f| f.read_only.clone()),
        Some(vec![std::path::PathBuf::from("/etc/moor-front")])
    );
    let e = err("fronts:\n  web:\n    read_only: [etc/moor-front]\n");
    assert_eq!(e.path, "fronts.web.read_only", "{e}");
    assert!(e.to_string().contains("must hold absolute paths"), "{e}");
}

#[test]
fn a_front_state_dir_under_tmp_is_refused() {
    let e = err("fronts:\n  web:\n    state_dir: /tmp/web\n");
    assert_eq!(e.path, "fronts.web.state_dir", "{e}");
    assert!(e.to_string().contains("must not lie under /tmp"), "{e}");
}

#[test]
fn a_front_read_only_path_under_tmp_is_refused() {
    for path in ["/tmp", "/tmp/x"] {
        let e = err(&format!("fronts:\n  web:\n    read_only: [{path}]\n"));
        assert_eq!(e.path, "fronts.web.read_only", "{e}");
        assert!(
            e.to_string().contains("must not be or lie under /tmp"),
            "{e}"
        );
    }
    parse_controller("c.yaml", "fronts:\n  web:\n    read_only: [/tmpfs/x]\n").expect("ok");
}

#[test]
fn a_front_lists_tcp_ports_and_no_port_zero() {
    let c = parse_controller("c.yaml", "fronts:\n  web:\n    ports: [8080, 8443]\n")
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(c.fronts.get("web").unwrap().ports, [8080, 8443]);
    assert!(parse_controller("c.yaml", "fronts:\n  web: {}\n")
        .unwrap()
        .fronts
        .get("web")
        .unwrap()
        .ports
        .is_empty());
    assert_eq!(
        err("fronts:\n  web:\n    ports: [0]\n").path,
        "fronts.web.ports"
    );
    assert_eq!(
        err("fronts:\n  web:\n    ports: [65536]\n").path,
        "fronts.web.ports[0]"
    );
}

#[test]
fn setup_failed_close_defaults_to_a_day_and_zero_turns_it_off() {
    assert_eq!(
        ControllerConfig::default().close_setup_failed_after(),
        Some(std::time::Duration::from_hours(24))
    );
    let off = parse_controller("c.yaml", "close_setup_failed_after_secs: 0\n")
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(off.close_setup_failed_after(), None);
}

#[test]
fn auto_close_defaults_to_a_day_after_a_run_and_a_week_idle_and_zero_turns_it_off() {
    let on = ControllerConfig::default()
        .auto_close()
        .expect("on by default");
    assert_eq!(on.every, std::time::Duration::from_mins(1));
    assert_eq!(on.run_after, Some(std::time::Duration::from_hours(24)));
    assert_eq!(on.idle_after, Some(std::time::Duration::from_hours(24 * 7)));
    let fast = parse_controller(
        "c.yaml",
        "auto_close_every_secs: 2\nauto_close_run_after_secs: 9\nauto_close_idle_after_secs: 0\n",
    )
    .unwrap_or_else(|e| panic!("{e}"))
    .auto_close()
    .expect("on");
    assert_eq!(fast.every, std::time::Duration::from_secs(2));
    assert_eq!(fast.run_after, Some(std::time::Duration::from_secs(9)));
    assert_eq!(fast.idle_after, None);
    let off =
        parse_controller("c.yaml", "auto_close_every_secs: 0\n").unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(off.auto_close(), None);
}

#[test]
fn workflow_floor_parses_and_defaults_to_an_hour() {
    let c = parse_controller("c.yaml", "workflow_min_interval_secs: 60\n")
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        c.workflow_min_interval(),
        std::time::Duration::from_secs(60)
    );
    assert_eq!(
        ControllerConfig::default().workflow_min_interval(),
        std::time::Duration::from_secs(3600)
    );
    assert_eq!(
        err("workflow_min_interval_secs: 0\n").path,
        "workflow_min_interval_secs"
    );
}

#[test]
fn a_remote_front_needs_ledger_tls_and_takes_no_command() {
    let remote = "fronts:\n  herdr:\n    remote:\n      key_file: /k\n";
    let err = parse_controller("c.yaml", remote).unwrap_err().to_string();
    assert!(err.contains("fronts.herdr.remote"), "{err}");
    assert!(err.contains("needs ledger_tls"), "{err}");
    let tls = "ledger_tls:\n  listen: 127.0.0.1:0\n  cert: /c\n  key: /k\n  client_ca: /a\n";
    let both =
        format!("{tls}fronts:\n  herdr:\n    command: [x]\n    remote:\n      key_file: /k\n");
    let err = parse_controller("c.yaml", &both).unwrap_err().to_string();
    assert!(err.contains("takes no command"), "{err}");
    let fine = format!("{tls}{remote}");
    assert!(parse_controller("c.yaml", &fine).is_ok());
}

#[test]
fn portal_url_is_an_origin_read_without_its_trailing_slash() {
    let c = parse_controller("c.yaml", "portal_url: https://moor.example.com/\n")
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        c.portal_origin().as_deref(),
        Some("https://moor.example.com")
    );
    assert_eq!(ControllerConfig::default().portal_origin(), None);
    for bad in [
        "moor.example.com",
        "https://",
        "https://moor.example.com/portal",
        "ftp://x",
    ] {
        assert_eq!(
            err(&format!("portal_url: {bad}\n")).path,
            "portal_url",
            "{bad}"
        );
    }
}
