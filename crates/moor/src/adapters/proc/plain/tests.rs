use super::last_sentence;

#[test]
fn crabbox_refusal_passes_whole() {
    let said = "provider=external refuses repository-configured external.connection.ssh.trustProviderOutput with inherited credentials; set the same provider-output contract in trusted user config to explicitly approve the credential destination";
    assert_eq!(
        last_sentence(&format!("note\n{said}\n\n")).as_deref(),
        Some(said)
    );
}

#[test]
fn tokens_keys_and_hosts_never_pass() {
    let got = last_sentence(
        "auth failed for token=ghp_A1b2C3d4E5f6G7h8 at https://box.example.com/x?k=v key AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl exit 2",
    )
    .unwrap();
    assert_eq!(got, "auth failed for … at … key … exit 2");
}

#[test]
fn a_line_of_nothing_plain_is_none() {
    assert_eq!(last_sentence("ghp_A1b2C3d4E5f6 10.0.0.9:22x"), None);
    assert_eq!(last_sentence(""), None);
}
