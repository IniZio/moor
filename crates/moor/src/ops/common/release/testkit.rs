//! Fixtures that print what a release script prints, for tests that drive the release op.

/// The nonce the `i`th release script gets: the fake RNG draws once per script.
pub(crate) fn nonce(i: u32) -> String {
    format!(
        "{:08x}",
        i.wrapping_mul(0x0001_0001).wrapping_add(0xab_cdef)
    )
}

/// What crabbox's run transcript looks like around a script's own stdout.
pub(crate) const BANNER: &str = "static SSH architecture observed=arm64 source=ssh-uname-sysctl\n\
workspace owner acquired wait=98ms recovered=false\n\
run context:\n\
running on 100.64.0.35 sh -c 'printf %s \"$HOME\"'\n\
/Users/newmancommand complete in 727ms total=1.23s\n";

/// `text` as the `i`th script prints it: between its markers, with banner text around it.
pub(crate) fn framed(i: u32, text: &str) -> String {
    let n = nonce(i);
    format!("{BANNER}MOOR_RELEASE_BEGIN {n}\n{text}\nMOOR_RELEASE_END {n}\nrun summary\n")
}
