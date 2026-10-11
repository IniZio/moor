use crate::fake::Rule;
use crate::world::World;

pub fn install(w: &World) {
    w.add_rules("claude", [Rule::on(&["stop"]).exit(0)]);
}
