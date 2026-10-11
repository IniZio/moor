pub mod claude;
pub mod crabbox;
pub mod herdr;
pub mod ssh;
mod template;

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

pub use template::{render, shell_quote, TemplateCtx};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Match {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub argv_prefix: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub argv_contains: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env_has: Option<Vec<String>>,
}

impl Match {
    pub fn matches(&self, argv: &[String], env_has: &dyn Fn(&str) -> bool) -> bool {
        if let Some(prefix) = &self.argv_prefix {
            if argv.len() < prefix.len() || argv[..prefix.len()] != prefix[..] {
                return false;
            }
        }
        if let Some(needles) = &self.argv_contains {
            if !needles
                .iter()
                .all(|n| argv.iter().any(|a| a.contains(n.as_str())))
            {
                return false;
            }
        }
        if let Some(vars) = &self.env_has {
            if !vars.iter().all(|v| env_has(v)) {
                return false;
            }
        }
        true
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Reply {
    pub stdout: String,
    pub stderr: String,
    pub exit: i32,
    pub sleep_ms: u64,
    pub block: bool,
    pub exec: Option<String>,
    pub rewrite: Vec<(String, String)>,
    pub write_files: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Rule {
    #[serde(default)]
    pub id: String,
    #[serde(rename = "match", default)]
    pub matcher: Match,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub times: Option<u32>,
    #[serde(default)]
    pub capture_stdin: bool,
    #[serde(default)]
    pub reply: Reply,
}

impl Rule {
    pub fn on(prefix: &[&str]) -> Rule {
        Rule {
            matcher: Match {
                argv_prefix: Some(prefix.iter().map(|s| s.to_string()).collect()),
                ..Match::default()
            },
            ..Rule::default()
        }
    }

    pub fn any() -> Rule {
        Rule::default()
    }

    pub fn contains(mut self, needle: &str) -> Self {
        self.matcher
            .argv_contains
            .get_or_insert_with(Vec::new)
            .push(needle.to_string());
        self
    }

    pub fn env_has(mut self, var: &str) -> Self {
        self.matcher
            .env_has
            .get_or_insert_with(Vec::new)
            .push(var.to_string());
        self
    }

    pub fn times(mut self, n: u32) -> Self {
        self.times = Some(n);
        self
    }

    pub fn stdout(mut self, s: impl Into<String>) -> Self {
        self.reply.stdout = s.into();
        self
    }

    pub fn stderr(mut self, s: impl Into<String>) -> Self {
        self.reply.stderr = s.into();
        self
    }

    pub fn exit(mut self, code: i32) -> Self {
        self.reply.exit = code;
        self
    }

    pub fn sleep_ms(mut self, ms: u64) -> Self {
        self.reply.sleep_ms = ms;
        self
    }

    pub fn block(mut self) -> Self {
        self.reply.block = true;
        self
    }

    pub fn exec(mut self, script: impl Into<String>) -> Self {
        self.reply.exec = Some(script.into());
        self
    }

    pub fn rewrite(mut self, from: impl Into<String>, to: impl Into<String>) -> Self {
        self.reply.rewrite.push((from.into(), to.into()));
        self
    }

    pub fn write_file(mut self, path: impl Into<String>, content: impl Into<String>) -> Self {
        self.reply.write_files.insert(path.into(), content.into());
        self
    }

    pub fn capture_stdin(mut self) -> Self {
        self.capture_stdin = true;
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Call {
    pub seq: u64,
    pub tool: String,
    pub argv: Vec<String>,
    pub cwd: String,
    pub pid: u32,
    pub ppid: u32,
    pub stdin: String,
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub env_digests: BTreeMap<String, String>,
    #[serde(default)]
    pub rule: Option<String>,
    #[serde(default)]
    pub pgid: u32,
    #[serde(default)]
    pub starttime: u64,
    #[serde(default)]
    pub env_contains_secret: bool,
}

pub fn digest(value: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(value.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

impl Call {
    pub fn flag(&self, name: &str) -> Option<String> {
        flag_value(&self.argv, name)
    }
}

fn flag_value(argv: &[String], name: &str) -> Option<String> {
    let eq = format!("{name}=");
    let mut i = 0;
    while i < argv.len() {
        if argv[i] == name {
            return argv.get(i + 1).cloned();
        }
        if let Some(v) = argv[i].strip_prefix(&eq) {
            return Some(v.to_string());
        }
        i += 1;
    }
    None
}

pub const RECORDED_ENV_PREFIXES: &[&str] = &[
    "MOOR_",
    "CRABBOX_",
    "GH_",
    "GIT_CONFIG_",
    "ANTHROPIC_",
    "CLAUDE_",
];

pub fn lock(fakes_dir: &Path) -> std::io::Result<fs::File> {
    let f = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(fakes_dir.join(".lock"))?;
    f.lock_exclusive()?;
    Ok(f)
}

pub fn rules_path(fakes_dir: &Path, tool: &str) -> PathBuf {
    fakes_dir.join(format!("{tool}.rules.json"))
}

pub fn secrets_path(fakes_dir: &Path) -> PathBuf {
    fakes_dir.join("secrets")
}

pub fn read_secrets(path: &Path) -> Vec<String> {
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

pub fn calls_path(fakes_dir: &Path, tool: &str) -> PathBuf {
    fakes_dir.join(format!("{tool}.jsonl"))
}

pub fn counts_path(fakes_dir: &Path, tool: &str) -> PathBuf {
    fakes_dir.join(format!("{tool}.counts.json"))
}

pub fn read_rules(fakes_dir: &Path, tool: &str) -> Vec<Rule> {
    match fs::read_to_string(rules_path(fakes_dir, tool)) {
        Ok(s) if !s.trim().is_empty() => {
            serde_json::from_str(&s).unwrap_or_else(|e| panic!("bad rules file for {tool}: {e}"))
        }
        _ => Vec::new(),
    }
}

fn write_atomic(path: &Path, content: &str) {
    let tmp = path.with_extension("tmp");
    let mut f = fs::File::create(&tmp).expect("create temp file");
    f.write_all(content.as_bytes()).expect("write temp file");
    f.sync_all().ok();
    fs::rename(&tmp, path).expect("rename temp file");
}

fn assign_ids(fakes_dir: &Path, rules: &mut [Rule]) {
    let path = fakes_dir.join(".ruleid");
    let mut k: u64 = fs::read_to_string(&path)
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0);
    for r in rules.iter_mut().filter(|r| r.id.is_empty()) {
        k += 1;
        r.id = format!("r{k}");
    }
    write_atomic(&path, &k.to_string());
}

fn store_rules(fakes_dir: &Path, tool: &str, mut new: Vec<Rule>, front: bool) {
    let _guard = lock(fakes_dir).expect("lock fakes dir");
    assign_ids(fakes_dir, &mut new);
    let mut all = read_rules(fakes_dir, tool);
    if front {
        new.extend(all);
        all = new;
    } else {
        all.extend(new);
    }
    let json = serde_json::to_string_pretty(&all).expect("serialize rules");
    write_atomic(&rules_path(fakes_dir, tool), &json);
}

pub fn append_rules(fakes_dir: &Path, tool: &str, rules: Vec<Rule>) {
    store_rules(fakes_dir, tool, rules, false);
}

pub fn prepend_rules(fakes_dir: &Path, tool: &str, rules: Vec<Rule>) {
    store_rules(fakes_dir, tool, rules, true);
}

fn parse_calls(path: &Path) -> Vec<Call> {
    let Ok(s) = fs::read_to_string(path) else {
        return Vec::new();
    };
    s.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("bad call line {l:?}: {e}")))
        .collect()
}

pub fn read_calls(fakes_dir: &Path, tool: &str) -> Vec<Call> {
    parse_calls(&calls_path(fakes_dir, tool))
}

pub fn all_calls(fakes_dir: &Path) -> Vec<Call> {
    let mut out = Vec::new();
    if let Ok(rd) = fs::read_dir(fakes_dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x == "jsonl") {
                out.extend(parse_calls(&p));
            }
        }
    }
    out.sort_by_key(|c| c.seq);
    out
}

pub fn next_seq_locked(fakes_dir: &Path) -> u64 {
    let path = fakes_dir.join("seq");
    let cur: u64 = fs::read_to_string(&path)
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0);
    let next = cur + 1;
    write_atomic(&path, &next.to_string());
    next
}

#[cfg(test)]
mod tests;
