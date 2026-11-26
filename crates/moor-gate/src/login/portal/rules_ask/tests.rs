use moor_core::name::ForgeRepo;

use super::{prompt, Asked};

fn asked(withheld: &[&str]) -> Asked {
    Asked {
        repo: "acme/web".to_owned(),
        words: "  let Boxes read the signed-in user  ".to_owned(),
        text: "rules:\n  - hosts: [api.github.com]\n".to_owned(),
        withheld: withheld.iter().map(|w| (*w).to_owned()).collect(),
    }
}

#[test]
fn the_prompt_leads_with_the_words_and_carries_the_skill_and_the_rules_whole() {
    let said = prompt(
        &ForgeRepo::new("acme", "web"),
        &asked(&["GET /user api.github.com"]),
    );
    assert!(
        said.starts_with("let Boxes read the signed-in user\n\n"),
        "{said}"
    );
    assert!(said.contains("Proxy rules of acme/web"), "{said}");
    assert!(said.contains("Do not save them"), "{said}");
    assert!(said.contains("- GET /user api.github.com"), "{said}");
    assert!(said.contains("# Writing a repo's Proxy rules"), "{said}");
    assert!(!said.contains("name: proxy-rules"), "front matter: {said}");
    assert!(
        said.ends_with("```yaml\nrules:\n  - hosts: [api.github.com]\n```\n"),
        "{said}"
    );
}

#[test]
fn no_withheld_request_says_none() {
    let said = prompt(&ForgeRepo::new("acme", "web"), &asked(&[]));
    assert!(said.contains("lists as withheld:\nnone\n"), "{said}");
}
