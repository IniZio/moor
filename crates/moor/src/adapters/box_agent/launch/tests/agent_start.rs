//! What a Box holds when its agent starts: every file its brief names, after the launch's steps
//! and the script's cleanup have run.

use std::os::unix::fs::PermissionsExt;

use moor_core::contract::skills;

use super::contract::run_req;
use super::*;

/// Every absolute path the brief names in backticks.
fn named_paths(brief: &str) -> Vec<String> {
    brief
        .split('`')
        .skip(1)
        .step_by(2)
        .filter(|p| p.starts_with('/') && !p.contains(char::is_whitespace))
        .map(str::to_owned)
        .collect()
}

/// Launches `r` into a scratch Box, runs the agent script there with a stub agent, and returns each
/// path the brief names with what the agent could read there when it started.
fn seen_at_agent_start(r: LaunchReq) -> Vec<(String, Option<String>)> {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path().to_str().unwrap().to_owned();
    let home = format!("{root}/home");
    let environ = format!("PATH=/usr/bin\0HOME={home}\0LANG=C\0");
    let d = Arc::new(FakeDriver::new().peek_text("box-1", &environ_path(), &environ));
    d.acquire(&AcquireReq {
        pool_machine: None,
        provider: ProviderId::new("fake"),
        box_id: r.id,
        name: box_name("t1"),
        class: None,
        nonce: "n0".to_owned(),
        cache: moor_core::cache::CachePlan::none(),
    })
    .unwrap();
    let r = LaunchReq {
        workdir: BoxPath::under(&root, "work").unwrap(),
        ..r
    };
    agent(&d).launch(&r).unwrap();
    let (cred, _) = r
        .dirs_under(&BoxPath::under(&root, "home").unwrap())
        .unwrap();
    let mut brief = String::new();
    for (_, path, bytes, mode) in d.puts() {
        let p = std::path::Path::new(path.as_str());
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, &bytes).unwrap();
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode)).unwrap();
        if path.as_str() == format!("{}/brief.md", cred.as_str()) {
            brief = String::from_utf8(bytes).unwrap();
        }
    }
    let paths = named_paths(&brief);
    let bin = format!("{root}/bin");
    std::fs::create_dir_all(&bin).unwrap();
    let copies: String = paths
        .iter()
        .enumerate()
        .map(|(i, p)| format!("[ -r '{p}' ] && cat '{p}' > \"$MARKER.{i}\"\n"))
        .collect();
    moor_testkit::write_executable(
        std::path::Path::new(&format!("{bin}/claude")),
        format!("#!/bin/sh\n{copies}touch \"$MARKER\"\n"),
    )
    .unwrap();
    let script = format!("{}/agent.sh", cred.as_str());
    let status = Command::new("sh")
        .arg(&script)
        .env("PATH", format!("{bin}:/usr/bin:/bin"))
        .env("HOME", &home)
        .env("MARKER", format!("{root}/started"))
        .stdin(Stdio::null())
        .status()
        .unwrap();
    assert!(status.success(), "the agent script failed: {status}");
    assert!(
        std::path::Path::new(&format!("{root}/started")).exists(),
        "the agent never started"
    );
    paths
        .into_iter()
        .enumerate()
        .map(|(i, p)| {
            (
                p,
                std::fs::read_to_string(format!("{root}/started.{i}")).ok(),
            )
        })
        .collect()
}

#[test]
fn every_skill_file_a_brief_names_is_readable_when_the_agent_starts() {
    let task = LaunchReq {
        id: BoxId::new(97_301),
        nonce: "seen1".to_owned(),
        ..req()
    };
    let run = LaunchReq {
        id: BoxId::new(97_302),
        nonce: "seen2".to_owned(),
        ..run_req()
    };
    for r in [task, run] {
        let seen = seen_at_agent_start(r);
        let skills = skills().unwrap();
        assert!(!seen.is_empty(), "a work Box's brief names its skill files");
        for (path, read) in seen {
            let skill = skills
                .iter()
                .find(|s| path.ends_with(&format!("/{}/SKILL.md", s.name)))
                .unwrap_or_else(|| panic!("{path} is not a built-in skill's file"));
            assert_eq!(
                read.as_deref(),
                Some(skill.file),
                "{path}, named in the brief, is not readable when the agent starts"
            );
        }
    }
}
