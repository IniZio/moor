use crate::fake::Rule;
use crate::world::World;

pub fn install(w: &World) {
    w.add_rules(
        "crabbox",
        [
            Rule::on(&["run"])
                .contains("MOOR_WORKDIR")
                .stdout("MOOR_WORKDIR={box}/work\n"),
            Rule::on(&["run"]).exec("{post}"),
            Rule::on(&["warmup"]).stdout("leased cbx_x slug={flag:--slug}\n"),
            Rule::on(&["list"]).stdout("[]"),
            Rule::on(&["stop"]).exit(0),
        ],
    );
}

pub fn inventory(w: &World, names: &[&str]) {
    let items: Vec<serde_json::Value> = names
        .iter()
        .map(|n| serde_json::json!({ "name": n }))
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
    w.calls("crabbox")
        .into_iter()
        .rfind(|c| c.argv.first().map(String::as_str) == Some("warmup"))
        .and_then(|c| c.flag("--slug"))
}
