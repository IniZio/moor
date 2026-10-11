use std::path::Path;

use serde_json::Value;

use crate::adapter::StartOpts;
use crate::world::World;

pub(super) fn str_list(v: &Value, key: &str) -> Vec<String> {
    v.get(key)
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Set `box.provider` in the user config (JSON, which old moor reads as YAML).
pub(super) fn merge_box_provider(path: &Path, provider: &str) {
    let mut doc: Value = match std::fs::read_to_string(path) {
        Ok(s) => {
            serde_json::from_str(&s).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => serde_json::json!({}),
        Err(e) => panic!("read {}: {e}", path.display()),
    };
    let root = doc
        .as_object_mut()
        .unwrap_or_else(|| panic!("{} is not a JSON object", path.display()));
    let b = root
        .entry("box")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .unwrap_or_else(|| panic!("{}: `box` is not an object", path.display()));
    b.insert("provider".into(), Value::String(provider.to_string()));
    std::fs::write(path, doc.to_string())
        .unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
}

pub(super) fn start_args(w: &World, name: &str, opts: &StartOpts) -> Vec<String> {
    if opts.size.is_some() {
        panic!("old adapter: size is a repo .moor.yaml setting; set it via RepoConfig");
    }
    if let Some(provider) = &opts.provider {
        merge_box_provider(&w.config_path(), provider);
    }
    let mut args: Vec<String> = vec!["start".into(), name.into(), "--json".into()];
    let mut flag = |f: &str, v: Option<String>| {
        if let Some(v) = v {
            args.push(f.into());
            args.push(v);
        }
    };
    flag("--issue", opts.issue.clone());
    flag("--goal", opts.goal.clone());
    flag("--model", opts.model.clone());
    flag(
        "--worktree",
        opts.worktree
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned()),
    );
    args
}
