use super::{device, refusal, DEVICE};

#[test]
fn a_missing_device_is_refused_with_the_firmware_fix() {
    let f = refusal(DEVICE, None).unwrap();
    assert_eq!(f.detail, "/dev/kvm is missing, and every Box gets KVM");
    assert!(f.fix.contains("kvm_intel or kvm_amd"), "{f:?}");
    assert!(f.fix.contains("docs/rootless-docker.md#kvm"), "{f:?}");
}

#[test]
fn boxes_need_the_device_open_to_everyone() {
    assert_eq!(refusal(DEVICE, Some(0o666)), None);
    assert!(refusal(DEVICE, Some(0o606)).is_none());
    let f = refusal(DEVICE, Some(0o660)).unwrap();
    assert_eq!(
        f.detail,
        "/dev/kvm is mode 0660, and rootless Docker drops its group, so Boxes cannot open it"
    );
    assert!(f.fix.contains("MODE=\"0666\""), "{f:?}");
    assert!(f.fix.contains("docs/rootless-docker.md#kvm"), "{f:?}");
}

#[test]
fn the_checks_read_the_named_device_else_dev_kvm() {
    assert_eq!(device(None), "/dev/kvm");
    assert_eq!(device(Some("")), "/dev/kvm");
    assert_eq!(device(Some("/tmp/kvm")), "/tmp/kvm");
}
