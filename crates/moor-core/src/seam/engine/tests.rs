use super::*;

#[test]
fn errors_display() {
    assert_eq!(
        EngineError::Box(DriverError::NotFound).to_string(),
        "box: box not found"
    );
}
