//! The host boot id for `SystemProc`.

use std::fs;

/// Where Linux publishes an identifier that changes on every boot.
const BOOT_ID: &str = "/proc/sys/kernel/random/boot_id";

/// The current boot id; `None` where the host does not publish one.
pub(super) fn read() -> Option<String> {
    let text = fs::read_to_string(BOOT_ID).ok()?;
    let id = text.trim();
    (!id.is_empty()).then(|| id.to_owned())
}

#[cfg(test)]
mod tests {
    #[cfg(target_os = "linux")]
    #[test]
    fn linux_has_a_stable_boot_id() {
        let id = super::read().unwrap();
        assert!(!id.is_empty());
        assert_eq!(super::read().as_deref(), Some(id.as_str()));
    }
}
