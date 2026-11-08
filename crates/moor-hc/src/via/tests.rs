use super::{Link, Via};

fn via(exe: &str, box_name: &str) -> Via {
    Via {
        exe: exe.to_owned(),
        link: Link::Server("https://moor.example.com".to_owned()),
        box_name: box_name.to_owned(),
    }
}

#[test]
fn a_rendered_command_parses_back() {
    for v in [
        via("/usr/local/bin/moor-hc", "moor/fix-x"),
        via("/home/a b/it's/moor-hc", "moor/fix-x"),
        Via {
            link: Link::Socket("/run/moor c.sock".to_owned()),
            ..via("/bin/moor-hc", "solo")
        },
    ] {
        assert_eq!(Via::parse(&v.render()), Some(v.clone()), "{}", v.render());
    }
}

#[test]
fn the_rendered_command_is_shell_text() {
    assert_eq!(
        via("/opt/moor-hc", "moor/fix-x").render(),
        "'/opt/moor-hc' exec --server 'https://moor.example.com' 'moor/fix-x' --"
    );
}

#[test]
fn hand_written_quoting_parses_too() {
    let parsed = Via::parse(r#"moor-hc exec --server "https://h" moor/fix\-x --"#);
    assert_eq!(
        parsed,
        Some(Via {
            exe: "moor-hc".to_owned(),
            link: Link::Server("https://h".to_owned()),
            box_name: "moor/fix-x".to_owned(),
        })
    );
}

#[test]
fn commands_moor_hc_did_not_write_are_not_ours() {
    for target in [
        "ssh -p 2222 user@host",
        "fly ssh console -a app --pty=false -C",
        "'/bin/moor-hc' exec --server 'https://h' 'b'",
        "'/bin/moor' exec --server 'https://h' 'b' --",
        "'/bin/moor-hc' run --server 'https://h' 'b' --",
        "'/bin/moor-hc' exec --other 'https://h' 'b' --",
        "'/bin/moor-hc' exec --server 'https://h' 'b' -- extra",
        "'/bin/moor-hc exec --server h b --",
        "vps",
    ] {
        assert_eq!(Via::parse(target), None, "{target}");
    }
}
