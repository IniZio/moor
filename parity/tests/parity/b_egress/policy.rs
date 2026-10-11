use super::*;

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
            f.set_repo(cfg(&[entry]));
            let (_, o) = f.start("bad", None);
            assert_ne!(o.class, ExitClass::Ok, "{entry} must be rejected");
            assert!(o.stderr.contains(entry), "{entry} not named: {}", o.stderr);
        }
        assert!(f.warmups().is_empty(), "no Box acquired for a bad entry");
        f.set_repo(cfg(&["Example.COM"]));
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
        let f = fx(Preset::Sprites, false, json!({}), cfg(&entries));
        let name = f.start_ok("base", None);
        let list = sprites_list(&f.warmup()).expect("allow flag");
        assert_set(
            &list,
            &want(
                &PROXY_SERVED,
                &["example.com", "*.example.com", "a.example.com"],
            ),
        );
        f.finish(&name);
    }
);

parity!(
    proxy_active_box_agent_cannot_reach_api_hosts_directly,
    covers = ["B-99", "C-122"],
    tags = [],
    {
        let f = fx(Preset::Sprites, false, json!({}), cfg(&["a.example.com"]));
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
            f.set_repo(cfg(&[served]));
            let (_, o) = f.start("rej", None);
            assert_ne!(o.class, ExitClass::Ok, "{served} must be refused");
            assert!(
                o.stderr.contains(served),
                "{served} not named: {}",
                o.stderr
            );
        }
        assert_eq!(f.warmups().len(), 1, "no Box acquired for refused entries");
    }
);

parity!(
    sprites_policy_always_on,
    covers = ["B-100", "C-123"],
    tags = [],
    {
        let boxed = fx(Preset::Sprites, false, json!({}), cfg(&[]));
        let name = boxed.start_ok("bx", None);
        let list = sprites_list(&boxed.warmup()).expect("flag despite empty egress");
        assert_set(&list, &want(&PROXY_SERVED, &[]));
        boxed.finish(&name);

        let ns = fx(Preset::NamespaceDevbox, false, json!({}), cfg(&[]));
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
        let f = fx(Preset::Sprites, false, json!({}), cfg(&["a.example.com"]));
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
            let f = fx(preset, true, json!({}), cfg(&["a.example.com"]));
            let name = f.start_ok("oth", None);
            f.warmup();
            no_policy_anywhere(&f.w);
            f.finish(&name);
        }
    }
);

parity!(
    namespace_policy_needs_opt_in_and_matches_sprites_list,
    covers = ["B-101", "C-124"],
    tags = [],
    {
        let entries = ["a.example.com"];
        let ns = fx(Preset::NamespaceDevbox, true, json!({}), cfg(&entries));
        let name = ns.start_ok("nsop", None);
        let warm = ns.warmup();
        assert!(warm.flag(SPRITES_FLAG).is_none());
        let ns_set = ns_list(&warm).expect("egress env with opt-in");
        ns.finish(&name);

        let sp = fx(Preset::Sprites, false, json!({}), cfg(&entries));
        let name = sp.start_ok("spcmp", None);
        let sp_set = sprites_list(&sp.warmup()).expect("flag");
        sp.finish(&name);
        assert_eq!(ns_set, sp_set, "same list as sprites");

        let off = fx(Preset::NamespaceDevbox, false, json!({}), cfg(&entries));
        let (name, o) = off.start("nsoff", None);
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        assert!(!o.warnings.is_empty(), "unenforced namespace start warns");
        assert!(ns_list(&off.warmup()).is_none());
        off.finish(&name);

        let sp = fx(Preset::Sprites, true, json!({}), cfg(&entries));
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

fn env_of(f: &Fx) -> Outcome<EnvReport> {
    moor().env(&f.w, &f.repo.path, &EnvOpts::default())
}

parity!(
    env_egress_item_follows_provider_capability,
    covers = ["B-102a"],
    tags = [],
    {
        let f = fx(Preset::Sprites, false, json!({}), cfg(&["a.example.com"]));
        let (name, detail) = egress_row(&env_of(&f));
        assert_eq!(name, "allow");
        assert!(detail.contains("a.example.com"), "{detail}");
        assert!(detail.contains("github.com"), "{detail}");
        assert!(!detail.contains("api.anthropic.com"), "{detail}");
        let out = env_of(&f).stdout.to_lowercase();
        assert!(
            !out.contains("crabbox") && !out.contains("network-allow"),
            "{out}"
        );

        f.set_repo(cfg(&[]));
        let (name, _) = egress_row(&env_of(&f));
        assert_eq!(name, "allow", "Box agent gets a policy even when empty");

        let lc = fx(Preset::LocalContainer, false, json!({}), cfg(&[]));
        let (lc_name, none) = egress_row(&env_of(&lc));
        assert_eq!(lc_name, "policy");
        lc.set_repo(cfg(&["a.example.com", "b.example.com"]));
        let (_, with_entries) = egress_row(&env_of(&lc));
        assert_ne!(with_entries, none);
        assert!(with_entries.contains('2'), "{with_entries}");

        let ns = fx(
            Preset::NamespaceDevbox,
            true,
            json!({}),
            cfg(&["a.example.com"]),
        );
        let (ns_name, ns_detail) = egress_row(&env_of(&ns));
        assert_eq!(ns_name, "allow");
        assert!(ns_detail.contains("a.example.com"));
        assert!(env_of(&ns).ok().item("egress", "enforce").is_none());
        let off = fx(
            Preset::NamespaceDevbox,
            false,
            json!({}),
            cfg(&["a.example.com"]),
        );
        let (off_name, _) = egress_row(&env_of(&off));
        assert_eq!(off_name, "policy");
        assert!(env_of(&off).ok().item("egress", "enforce").is_some());
    }
);
