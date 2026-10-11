use parity::adapter::{EventKind, ExitClass, TaskState};
use parity::assert::assert_secret_absent;
use parity::fake::crabbox;
use parity::parity;
use serde_json::json;

use super::support::*;
use crate::moor;

parity!(
    start_rejects_unresolved_credential_and_mcp_refs,
    covers = ["C-41", "C-136"],
    tags = [],
    {
        let f = fx();
        flow_fakes(&f.w);
        f.repo_cfg(json!({ "credentials": ["MISSING"] }));
        let name = f.w.task_name("refcred");
        assert_rejected_start(&f, &start_task(&f, &name, None), &name);

        let f = fx();
        flow_fakes(&f.w);
        f.user(json!({
            "credentials": { "DEAD": { "file": "/nonexistent/x" } },
            "mcp_servers": { "srv": { "command": "x", "args": ["${DEAD}"] } },
        }));
        let name = f.w.task_name("mcpcred");
        assert_rejected_start(&f, &start_task(&f, &name, None), &name);

        let f = fx();
        flow_fakes(&f.w);
        write_file(
            &f.repo.path.join(".mcp.json"),
            &json!({ "mcpServers": { "a": {} } }).to_string(),
            0o644,
        );
        let name = f.w.task_name("mcpappr");
        assert_rejected_start(&f, &start_task(&f, &name, None), &name);
        assert!(
            !f.repo.path.join(".claude").exists(),
            "moor never writes the approval"
        );

        write_file(
            &f.repo.path.join(".claude/settings.local.json"),
            &json!({ "enabledMcpjsonServers": ["a"] }).to_string(),
            0o644,
        );
        let again = f.w.task_name("mcpok");
        started_ok(&f, &again, None);
        finish(&f, &again);
    }
);

parity!(
    start_ignores_unrelated_broken_credential,
    covers = [],
    tags = ["new_only(EnvPreflightScoped)"],
    {
        let f = fx();
        flow_fakes(&f.w);
        f.user(json!({ "credentials": { "UNRELATED": { "file": "/nonexistent/x" } } }));
        let name = f.w.task_name("unrel");
        let o = start_task(&f, &name, None);
        let state = o.value.as_ref().map(|t| t.state.clone());
        assert_eq!(
            state,
            Some(TaskState::Working),
            "start rejected: {}",
            o.stderr
        );
        finish(&f, &name);
    }
);

parity!(
    start_failures_print_outcome_json_on_stdout,
    covers = [],
    tags = [],
    {
        let f = fx();
        flow_fakes(&f.w);
        crabbox::fail_quota(&f.w);
        let quota = start_task(&f, &f.w.task_name("quota"), None);
        assert_eq!(quota.class, ExitClass::Failed);
        assert!(
            quota.value.is_some(),
            "acquire failure prints the outcome: {}",
            quota.stdout
        );

        let f = fx();
        flow_fakes(&f.w);
        f.repo_cfg(json!({ "credentials": ["MISSING"] }));
        let rejected = start_task(&f, &f.w.task_name("rej"), None);
        assert_eq!(rejected.class, ExitClass::Failed);
        assert!(
            rejected.value.is_some(),
            "rejection prints the outcome: {}",
            rejected.stdout
        );

        let f = fx();
        flow_fakes(&f.w);
        let blank = f.secret_file("blank", "  \n");
        f.user(json!({ "credentials": { "TOK": { "file": blank } } }));
        f.repo_cfg(json!({ "setup": ["true"], "box_credentials": ["TOK"] }));
        let name = f.w.task_name("blank");
        let setup = start_task(&f, &name, None);
        assert!(
            setup.value.is_some(),
            "credential read failure prints the outcome: {}",
            setup.stdout
        );
        finish(&f, &name);
    }
);

parity!(
    checked_event_and_registry_carry_names_only,
    covers = ["C-42"],
    tags = [],
    {
        let f = fx();
        flow_fakes(&f.w);
        let (tok, keyf) = (f.secret("tok"), f.secret("keyf"));
        creds_config(&f, &tok, &keyf);
        f.repo_cfg(json!({ "credentials": ["TOK"], "box_credentials": pair_credentials() }));
        let name = f.w.task_name("checked");
        started_ok(&f, &name, None);
        let events = moor().events(&f.w, &name);
        let checked = EventKind::Other("checked".into());
        let mut seen = 0;
        for e in &events {
            let text = e.payload.to_string();
            assert!(
                !text.contains(&tok) && !text.contains(&keyf),
                "{:?} carries a value",
                e.kind
            );
            if e.kind == checked {
                let list = e.payload["credentials"]
                    .as_array()
                    .expect("credentials list");
                assert!(!list.is_empty(), "checked event lists no credentials");
                seen += 1;
                for c in list {
                    let keys: Vec<&String> = c.as_object().expect("entry").keys().collect();
                    assert!(keys
                        .iter()
                        .all(|k| ["name", "source", "resolves"].contains(&k.as_str())));
                }
            }
        }
        assert_eq!(seen, 1, "one checked event expected: {events:?}");
        finish(&f, &name);
        for s in [&tok, &keyf] {
            assert_secret_absent(&f.w, s, &[]);
        }
    }
);
