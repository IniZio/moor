//! What tests hold moor's contract to: its locked version and the old Box texts it replaced.

/// The version `agents/distro/contract.lock` holds.
pub(crate) fn locked_version() -> u32 {
    include_str!("../../../../agents/distro/contract.lock")
        .lines()
        .find_map(|l| l.strip_prefix("version = "))
        .and_then(|v| v.trim().parse().ok())
        .expect("the contract lock names a version")
}

/// Sentences of the task and run Box texts the contract replaced, one from each deleted part.
pub(crate) const OLD_TEXT: &[&str] = &[
    "You are running inside the machine",
    "Report the command's real exit code",
    "`sudo -n` may work here",
    "If a tool the work needs is missing, install it",
    "Before you tell the person a tool or program is missing",
    "Do not add Co-Authored-By trailers",
    "When you opened a pull request, end your final message with its URL.",
    "You may receive follow-up messages (steers)",
    "Decide on your own when you reasonably can",
    "Reports are how you show work to a human",
    "To show a human a file",
    "attach a published file to a pull request",
    "Do not run `gh auth login`",
    "do not retry or look for a way around it",
    "After each write outside moor, such as a pull request comment",
    "Report only the gate that is in hand",
    "load it first if your tools",
    "moor tells you when each gate is in hand",
    "How the run ends is not yours to say",
    "Never fake evidence or a result",
    "If you find there is nothing to do",
    "Or call it with result fail",
];
