use super::{device, refusal, DEVICE};

#[test]
fn a_missing_device_is_refused_on_either_daemon_with_the_firmware_fix() {
    for rootless in [true, false] {
        let f = refusal(rootless, DEVICE, None).unwrap();
        assert_eq!(f.detail, "/dev/kvm is missing, and every Box gets KVM");
        assert!(f.fix.contains("kvm_intel or kvm_amd"), "{f:?}");
        assert!(f.fix.contains("docs/rootless-docker.md#kvm"), "{f:?}");
    }
}

#[test]
fn rootless_boxes_need_the_device_open_to_everyone() {
    assert_eq!(refusal(true, DEVICE, Some(0o666)), None);
    let f = refusal(true, DEVICE, Some(0o660)).unwrap();
    assert_eq!(
        f.detail,
        "/dev/kvm is mode 0660, and rootless Docker drops its group, so Boxes cannot open it"
    );
    assert!(f.fix.contains("MODE=\"0666\""), "{f:?}");
    assert!(f.fix.contains("docs/rootless-docker.md#kvm"), "{f:?}");
}

#[test]
fn rootful_boxes_need_the_device_open_to_its_group_or_to_everyone() {
    assert_eq!(refusal(false, DEVICE, Some(0o660)), None);
    assert_eq!(refusal(false, DEVICE, Some(0o606)), None);
    let f = refusal(false, DEVICE, Some(0o600)).unwrap();
    assert_eq!(
        f.detail,
        "/dev/kvm is mode 0600, so Boxes cannot open it even in its group"
    );
    assert!(f.fix.contains("docs/rootless-docker.md#kvm"), "{f:?}");
}

#[test]
fn the_checks_read_the_named_device_else_dev_kvm() {
    assert_eq!(device(None), "/dev/kvm");
    assert_eq!(device(Some("")), "/dev/kvm");
    assert_eq!(device(Some("/tmp/kvm")), "/tmp/kvm");
}
