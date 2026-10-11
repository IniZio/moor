use crate::fake::Rule;
use crate::world::World;

pub fn install(w: &World) {
    w.add_rules(
        "ssh",
        [Rule::any()
            .exec("cd '{box}' && {last}")
            .rewrite("/tmp/", "{box}/tmp/")],
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::read_rules;

    #[test]
    fn default_rule_redirects_tmp_into_box() {
        let w = World::new("/bin/true");
        install(&w);
        let rules = read_rules(&w.fakes_dir(), "ssh");
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].reply.exec.as_deref(), Some("cd '{box}' && {last}"));
        assert_eq!(
            rules[0].reply.rewrite,
            vec![("/tmp/".to_string(), "{box}/tmp/".to_string())]
        );
    }
}
