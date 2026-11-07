//! Prelude lines that point build tools at their SDK and cache.

/// Where the `android-sdk` cache preset keeps the Android SDK.
const ANDROID_SDK: &str = "/opt/android-sdk";

/// Lines that point Android tools at the SDK in `root`, a path with no shell metacharacters, for a variable the repo did not set and
/// only when `root` exists, so a Box without the volume keeps finding its SDK where it did.
pub(super) fn android_lines(root: &str) -> Vec<String> {
    vec![format!(
        "if [ -d {root} ]; then export ANDROID_HOME=\"${{ANDROID_HOME:-{root}}}\" ANDROID_SDK_ROOT=\"${{ANDROID_SDK_ROOT:-{root}}}\"; fi"
    )]
}

/// Lines that make compiles launched by cmake use ccache when the Box has it, for a launcher the
/// repo or user did not set. A set but empty value is kept, which cmake reads as no launcher.
/// Without the check, a Box whose image lacks ccache would fail every configure.
pub(super) fn ccache_lines() -> Vec<String> {
    let exports = ["CMAKE_C_COMPILER_LAUNCHER", "CMAKE_CXX_COMPILER_LAUNCHER"]
        .map(|name| format!("[ -n \"${{{name}+x}}\" ] || export {name}=ccache;"));
    vec![format!(
        "if command -v ccache >/dev/null 2>&1; then {} fi",
        exports.join(" ")
    )]
}

/// Both sets of lines, in the order the prelude runs them.
pub(super) fn lines() -> Vec<String> {
    let mut lines = android_lines(ANDROID_SDK);
    lines.extend(ccache_lines());
    lines
}
