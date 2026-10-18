use super::*;

#[test]
fn label_is_moor_prefixed() {
    let name = BoxName::parse_allow_bare("fix-bug").unwrap();
    assert_eq!(workspace_label(&name), "moor:fix-bug");
}

#[test]
fn refs_round_trip() {
    assert_eq!(WorkspaceRef::new("w1").as_str(), "w1");
    assert_eq!(PaneRef::new(String::from("p1")).as_str(), "p1");
}

#[test]
fn errors_display() {
    assert_eq!(SurfaceError::NotFound.to_string(), "not found");
    assert_eq!(
        SurfaceError::Failed {
            summary: "x".into()
        }
        .to_string(),
        "surface call failed: x"
    );
}
