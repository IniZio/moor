use crate::fake::Rule;
use crate::world::World;

pub fn install(w: &World) {
    w.add_rules(
        "claude",
        [
            Rule::any().contains("--bg").stdout("backgrounded · a{n}\n"),
            Rule::on(&["agents"])
                .stdout(r#"[{"id":"a1","sessionId":"a1-full","cwd":"","state":"working"}]"#),
            Rule::on(&["stop"]).exit(0),
        ],
    );
}

pub fn agents(w: &World, json: &str) {
    w.prepend_rules("claude", [Rule::on(&["agents"]).stdout(json)]);
}
