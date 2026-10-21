use moor_core::seam::forge::{Forge, ForgeRepo};

use super::GitHub;

#[test]
fn repo_of_delegates() {
    assert_eq!(
        GitHub.repo_of("git@github.com:o/n.git"),
        Some(ForgeRepo::new("o", "n"))
    );
}
