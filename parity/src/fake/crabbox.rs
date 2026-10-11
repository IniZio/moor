use crate::fake::{Call, Rule};
use crate::world::World;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preset {
    LocalContainer,
    NamespaceDevbox,
    Sprites,
    Ssh,
}

impl Preset {
    pub fn provider(self) -> &'static str {
        match self {
            Preset::LocalContainer => "local-container",
            Preset::NamespaceDevbox => "namespace-devbox",
            Preset::Sprites => "sprites",
            Preset::Ssh => "ssh",
        }
    }
}

const NO_JSON: &str = "flag provided but not defined: -json\n";
const INSPECT_JSON: &str = "{\"id\":\"{flag:--id}\",\"slug\":\"{flag:--id}\",\"provider\":\"{flag:--provider}\",\"target\":\"linux\",\"workroot\":\"{box}/work\",\"state\":\"active\",\"serverId\":\"1\",\"serverType\":\"fake\",\"host\":\"127.0.0.1\",\"network\":\"public\",\"sshHost\":\"127.0.0.1\",\"sshUser\":\"crabbox\",\"sshPort\":\"22\",\"sshKey\":\"\",\"hasHost\":true,\"ready\":true}\n";

pub fn install(w: &World) {
    install_preset(w, Preset::LocalContainer);
}

pub fn install_preset(w: &World, p: Preset) {
    install_defaults(w);
    match p {
        Preset::NamespaceDevbox => pause_resume(w),
        Preset::Sprites => w.add_rules(
            "crabbox",
            [Rule::on(&["resume"]).exit(1).stderr("resume unsupported\n")],
        ),
        Preset::LocalContainer | Preset::Ssh => {}
    }
    providers(w);
}

fn pause_resume(w: &World) {
    pause(w);
    resume(w);
}

fn install_defaults(w: &World) {
    w.add_rules(
        "crabbox",
        [
            Rule::on(&["run"])
                .contains("MOOR_WORKDIR")
                .stdout("MOOR_WORKDIR={box}/work\n"),
            Rule::on(&["run"]).exec("{post}"),
            Rule::on(&["warmup"])
                .contains("--json")
                .exit(2)
                .stderr(NO_JSON),
            Rule::on(&["warmup"]).stdout("leased cbx_x slug={flag:--slug}\n"),
            Rule::on(&["list"]).stdout("[]"),
            Rule::on(&["stop"])
                .contains("--json")
                .exit(2)
                .stderr(NO_JSON),
            Rule::on(&["stop"]).exit(0),
            Rule::on(&["inspect"])
                .contains("--json")
                .stdout(INSPECT_JSON),
        ],
    );
}

pub fn inventory(w: &World, names: &[&str]) {
    let items: Vec<serde_json::Value> = names
        .iter()
        .enumerate()
        .map(|(i, n)| {
            serde_json::json!({
                "id": i + 1, "name": n, "status": "running",
                "labels": { "slug": n, "lease": format!("cbx_{n}") },
                "public_net": { "ipv4": { "ip": "127.0.0.1" } },
            })
        })
        .collect();
    let body = serde_json::Value::Array(items).to_string();
    w.prepend_rules("crabbox", [Rule::on(&["list"]).stdout(body)]);
}

pub fn fail_quota(w: &World) {
    w.prepend_rules(
        "crabbox",
        [Rule::on(&["warmup"])
            .exit(2)
            .stderr("quota exceeded for plan\n")],
    );
}

pub fn slug(w: &World) -> Option<String> {
    call(w, "warmup").and_then(|c| c.flag("--slug"))
}

pub fn providers(w: &World) {
    let entry = |p: &str, life: &str| {
        serde_json::json!({
            "provider": p, "family": p, "aliases": [], "kind": "ssh-lease",
            "targets": ["linux"], "features": ["ssh"], "lifecycle": [life],
            "coordinator": "never", "classCatalog": {},
        })
    };
    let body = serde_json::json!([
        entry("local-container", "ephemeral"),
        entry("namespace-devbox", "pause-resume"),
        entry("sprites", "ephemeral"),
        entry("ssh", "static"),
    ])
    .to_string();
    w.prepend_rules("crabbox", [Rule::on(&["providers"]).stdout(body)]);
}

pub fn not_found(w: &World) {
    w.prepend_rules(
        "crabbox",
        [Rule::on(&["stop"]).exit(4).stderr("not found\n")],
    );
}

pub fn fail(w: &World, verb: &str) {
    w.prepend_rules(
        "crabbox",
        [Rule::on(&[verb]).exit(1).stderr(format!("{verb} failed\n"))],
    );
}

pub fn gone(w: &World, box_name: &str) {
    let current = crate::fake::read_rules(&w.fakes_dir(), "crabbox")
        .into_iter()
        .find(|r| r.matcher.argv_prefix.as_deref() == Some(&["list".to_string()][..]))
        .and_then(|r| serde_json::from_str::<Vec<serde_json::Value>>(&r.reply.stdout).ok())
        .unwrap_or_default();
    let kept: Vec<_> = current
        .into_iter()
        .filter(|i| i["name"].as_str() != Some(box_name))
        .collect();
    let body = serde_json::Value::Array(kept).to_string();
    w.prepend_rules("crabbox", [Rule::on(&["list"]).stdout(body)]);
}

const CP_SCRIPT: &str = "src='{argv[5]}'; cp -R \"${src#SANDBOX:}\" '{argv[6]}'";

pub fn cp(w: &World) {
    w.add_rules("crabbox", [Rule::on(&["cp"]).exec(CP_SCRIPT)]);
}

pub fn fail_match(w: &World, verb: &str, pat: &str, code: i32) {
    w.prepend_rules(
        "crabbox",
        [Rule::on(&[verb])
            .contains(pat)
            .exit(code)
            .stderr(format!("{verb} failed\n"))],
    );
}

pub fn block(w: &World, verb: &str) {
    w.prepend_rules("crabbox", [Rule::on(&[verb]).block()]);
}

pub fn die_on_run(w: &World) {
    w.prepend_rules("crabbox", [Rule::on(&["run"]).exec("kill -KILL $PPID")]);
}

pub fn run_in(w: &World, dir: &std::path::Path) {
    let script = format!(
        "cd '{}' && GIT_CEILING_DIRECTORIES='{}' {{post}}",
        dir.display(),
        w.root().display()
    );
    w.prepend_rules("crabbox", [Rule::on(&["run"]).exec(script)]);
}

pub fn pause(w: &World) {
    w.add_rules("crabbox", [Rule::on(&["pause"]).exit(0)]);
}

pub fn resume(w: &World) {
    w.add_rules("crabbox", [Rule::on(&["resume"]).exit(0)]);
}

pub fn connect(w: &World) {
    w.add_rules("crabbox", [Rule::on(&["connect"]).exit(0)]);
}

pub fn call(w: &World, verb: &str) -> Option<Call> {
    w.calls("crabbox")
        .into_iter()
        .rfind(|c| c.argv.first().map(String::as_str) == Some(verb))
}

pub fn ssh_wrapper(w: &World) {
    w.prepend_rules("crabbox", [Rule::on(&["ssh"]).stdout("'{root}/bin/ssh'\n")]);
}

pub fn ssh_channel(w: &World) {
    w.prepend_rules(
        "crabbox",
        [Rule::on(&["ssh"]).stdout("{root}/bin/ssh -o BatchMode=yes box\n")],
    );
    crate::fake::ssh::install(w);
}

pub fn tunnel(w: &World) {
    w.prepend_rules(
        "crabbox",
        [Rule::on(&["tunnel"])
            .stdout("{\"box\":20000,\"host\":20000}\n")
            .block()],
    );
}

pub fn deny(w: &World, verb: &str, code: i32) {
    w.prepend_rules("crabbox", [Rule::on(&[verb]).exit(code)]);
}
