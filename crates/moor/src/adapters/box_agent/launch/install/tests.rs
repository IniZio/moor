use std::os::unix::fs::PermissionsExt;
use std::process::Command;

use moor_core::seam::driver::BoxStep;

use super::platform_step;

fn mode(path: &std::path::Path) -> String {
    format!(
        "{:o}",
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    )
}

#[test]
fn the_platform_read_leaves_the_credential_dir_and_its_file_owner_only() {
    let home = tempfile::tempdir().unwrap();
    let step = platform_step(".moor-cred-t1-x");
    let BoxStep::Run(script) = &step else {
        return assert!(matches!(step, BoxStep::Run(_)), "a script step");
    };
    let out = Command::new("sh")
        .args(["-c", &format!("umask 022; {script}")])
        .env("HOME", home.path())
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    let dir = home.path().join(".moor-cred-t1-x");
    assert_eq!(mode(&dir), "700");
    assert_eq!(mode(&dir.join("uname")), "600");
}
