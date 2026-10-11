use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use parity::fake::{
    calls_path, counts_path, lock, next_seq_locked, read_rules, render, Call, Rule, TemplateCtx,
    RECORDED_ENV_PREFIXES,
};

const STDIN_CAP: usize = 64 * 1024;

fn locate_argv0(argv0: &Path) -> PathBuf {
    if argv0.components().count() > 1 {
        return argv0.to_path_buf();
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    for dir in std::env::split_paths(&path) {
        let cand = dir.join(argv0);
        if cand.exists() {
            return cand;
        }
    }
    argv0.to_path_buf()
}

fn read_stdin(capture_all: bool) -> String {
    if capture_all {
        let mut buf = Vec::new();
        let _ = std::io::stdin().read_to_end(&mut buf);
        return String::from_utf8_lossy(&buf).into_owned();
    }
    if unsafe { libc::isatty(0) } == 1 {
        return String::new();
    }
    let shared = Arc::new(Mutex::new(Vec::<u8>::new()));
    let (tx, rx) = mpsc::channel::<()>();
    let writer = Arc::clone(&shared);
    std::thread::spawn(move || {
        let mut stdin = std::io::stdin();
        let mut chunk = [0u8; 4096];
        loop {
            match stdin.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(k) => {
                    let mut buf = writer.lock().unwrap();
                    let room = STDIN_CAP.saturating_sub(buf.len());
                    buf.extend_from_slice(&chunk[..k.min(room)]);
                    if buf.len() >= STDIN_CAP {
                        break;
                    }
                }
            }
        }
        let _ = tx.send(());
    });
    let _ = rx.recv_timeout(Duration::from_millis(200));
    let buf = shared.lock().unwrap();
    String::from_utf8_lossy(&buf).into_owned()
}

fn recorded_env() -> BTreeMap<String, String> {
    std::env::vars()
        .filter(|(k, _)| {
            k == "PATH" || k == "HOME" || RECORDED_ENV_PREFIXES.iter().any(|p| k.starts_with(p))
        })
        .collect()
}

fn run() -> i32 {
    let mut args = std::env::args_os();
    let argv0 = PathBuf::from(args.next().unwrap_or_default());
    let argv: Vec<String> = args.map(|a| a.to_string_lossy().into_owned()).collect();
    let tool = argv0
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let fakes_dir = match std::env::var_os("PARITY_FAKES") {
        Some(v) => PathBuf::from(v),
        None => locate_argv0(&argv0)
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("../fakes"),
    };
    if !fakes_dir.is_dir() {
        eprintln!("fake {tool}: fakes dir missing: {}", fakes_dir.display());
        return 0;
    }
    let root = fakes_dir.parent().unwrap_or(&fakes_dir).to_path_buf();
    let box_root = root.join("box");
    let cwd = std::env::current_dir().unwrap_or_default();

    let guard = match lock(&fakes_dir) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("fake {tool}: lock failed: {e}");
            return 0;
        }
    };
    let rules = read_rules(&fakes_dir, &tool);
    let cpath = counts_path(&fakes_dir, &tool);
    let mut counts: BTreeMap<String, u32> = std::fs::read(&cpath)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    let env_has = |name: &str| std::env::var_os(name).is_some();
    let rule: Option<&Rule> = rules.iter().find(|r| {
        r.matcher.matches(&argv, &env_has)
            && r.times
                .is_none_or(|t| counts.get(&r.id).copied().unwrap_or(0) < t)
    });
    let mut n = 0;
    if let Some(r) = rule {
        let c = counts.entry(r.id.clone()).or_insert(0);
        *c += 1;
        n = *c;
        if let Ok(s) = serde_json::to_vec(&counts) {
            let _ = std::fs::write(&cpath, s);
        }
    }

    let stdin = read_stdin(rule.is_some_and(|r| r.capture_stdin));
    let call = Call {
        seq: next_seq_locked(&fakes_dir),
        tool: tool.clone(),
        argv: argv.clone(),
        cwd: cwd.to_string_lossy().into_owned(),
        pid: std::process::id(),
        ppid: unsafe { libc::getppid() } as u32,
        stdin,
        env: recorded_env(),
        rule: rule.map(|r| r.id.clone()),
    };
    if let Ok(line) = serde_json::to_string(&call) {
        let file = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(calls_path(&fakes_dir, &tool));
        if let Ok(mut f) = file {
            let _ = writeln!(f, "{line}");
        }
    }
    drop(guard);

    let Some(rule) = rule else { return 0 };
    let reply = &rule.reply;
    let ctx = TemplateCtx {
        argv: &argv,
        n,
        box_root: &box_root,
        root: &root,
        cwd: &cwd,
    };
    for (path, content) in &reply.write_files {
        let path = PathBuf::from(render(path, &ctx));
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&path, render(content, &ctx));
    }
    if reply.sleep_ms > 0 {
        std::thread::sleep(Duration::from_millis(reply.sleep_ms));
    }
    print!("{}", render(&reply.stdout, &ctx));
    eprint!("{}", render(&reply.stderr, &ctx));
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();

    let mut code = reply.exit;
    if let Some(script) = &reply.exec {
        let mut script = render(script, &ctx);
        for (from, to) in &reply.rewrite {
            script = script.replace(from, to);
        }
        let status = Command::new("sh")
            .arg("-c")
            .arg(&script)
            .env("HOME", box_root.join("home"))
            .status();
        let child = match status {
            Ok(s) => s.code().unwrap_or_else(|| 128 + s.signal().unwrap_or(0)),
            Err(e) => {
                eprintln!("fake {tool}: exec failed: {e}");
                127
            }
        };
        if code == 0 {
            code = child;
        }
    }
    if reply.block {
        loop {
            std::thread::sleep(Duration::from_secs(3600));
        }
    }
    code
}

fn main() {
    std::process::exit(run());
}
