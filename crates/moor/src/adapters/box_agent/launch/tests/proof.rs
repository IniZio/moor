//! A launch puts the proof publish helper in the Box.

use super::*;
use moor_core::boxscript::proof_publish;

#[test]
fn delivers_the_proof_publish_helper_with_its_script() {
    let (d, _, out) = launch(FakeDriver::new(), FakeSurface::new("/r"));
    out.unwrap();
    let puts = d.puts();
    let put = puts
        .iter()
        .find(|p| p.1.as_str() == format!("/work/box-1/{}", proof_publish::FILE))
        .expect("helper put");
    assert_eq!(put.2, proof_publish::SCRIPT.as_bytes());
    assert_eq!(put.3, 0o600);
}

#[test]
fn a_failed_helper_write_is_a_proof_failure() {
    let (_, s, out) = launch(
        FakeDriver::new().fail_put_after(1, DriverError::NotFound),
        FakeSurface::new("/r"),
    );
    assert!(matches!(
        out,
        Err(EngineError::Failed { step: "proof", .. })
    ));
    assert!(s.opened().is_empty());
}
