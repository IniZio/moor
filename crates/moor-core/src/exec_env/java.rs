//! Prelude lines that point Java and Gradle at the Proxy and its CA.

use crate::name::DirectHost;

/// Shell variables `java_lines` sets; the prelude unsets them at its end.
pub(super) const JAVA_SHELL_VARS: &str = "moor_g moor_p moor_ts moor_jh";

/// Marker comments around the block of `gradle.properties` that moor owns.
const BEGIN: &str = "# moor proxy: begin";
/// See [`BEGIN`].
const END: &str = "# moor proxy: end";

/// Hosts the JVM always reaches directly, not through the Proxy.
const NON_PROXY: &str = "127.0.0.1|localhost";

/// Lines that give the JVM the Proxy and a trust store holding its CA.
///
/// `gradle.properties` is rewritten under `umask 077`, since it may hold the user's own
/// secrets. Without `keytool`, or if any step fails, no Java variable is set.
pub(super) fn java_lines(box_port: u16, ca: &str, direct: &[DirectHost]) -> Vec<String> {
    let non_proxy: String = std::iter::once(NON_PROXY.to_owned())
        .chain(direct.iter().map(DirectHost::java))
        .collect::<Vec<_>>()
        .join("|");
    let mut lines = vec![
        "if command -v keytool >/dev/null 2>&1; then".to_owned(),
        "moor_g=\"${GRADLE_USER_HOME:-$HOME/.gradle}\"; moor_p=\"$moor_g/gradle.properties\"; moor_ts=\"$moor_g/moor-proxy-truststore.p12\"".to_owned(),
        "moor_jh=\"$JAVA_HOME/lib/security/cacerts\"".to_owned(),
        "[ -f \"$moor_jh\" ] || moor_jh=\"$(dirname \"$(dirname \"$(readlink -f \"$(command -v keytool)\")\")\")/lib/security/cacerts\"".to_owned(),
        "if mkdir -p \"$moor_g\" 2>/dev/null &&".to_owned(),
        "rm -f \"$moor_ts.$$\" &&".to_owned(),
        "{ [ ! -f \"$moor_jh\" ] || keytool -importkeystore -srckeystore \"$moor_jh\" -destkeystore \"$moor_ts.$$\" -deststoretype PKCS12 -deststorepass changeit -srcstorepass changeit -noprompt >/dev/null 2>&1; } &&".to_owned(),
        format!("keytool -importcert -noprompt -alias moor-proxy -file {ca} -keystore \"$moor_ts.$$\" -storetype PKCS12 -storepass changeit >/dev/null 2>&1 &&"),
        "mv -f \"$moor_ts.$$\" \"$moor_ts\" &&".to_owned(),
        "( umask 077; {".to_owned(),
        format!("[ ! -f \"$moor_p\" ] || sed '/^{BEGIN}$/,/^{END}$/d' \"$moor_p\" 2>/dev/null || exit 1; [ -z \"$(tail -c1 \"$moor_p\" 2>/dev/null)\" ] || printf '\\n'; printf '%s\\n' '{BEGIN}'"),
    ];
    for scheme in ["http", "https"] {
        lines.push(format!(
            "printf '%s\\n' 'systemProp.{scheme}.proxyHost=127.0.0.1' 'systemProp.{scheme}.proxyPort={box_port}'"
        ));
    }
    lines.extend(
        [
            format!("printf '%s\\n' 'systemProp.http.nonProxyHosts={non_proxy}' 'systemProp.https.nonProxyHosts={non_proxy}' \"systemProp.javax.net.ssl.trustStore=$moor_ts\" 'systemProp.javax.net.ssl.trustStorePassword=changeit' 'systemProp.javax.net.ssl.trustStoreType=PKCS12' '{END}'"),
            "} >\"$moor_p.$$\" ) &&".to_owned(),
            "mv -f \"$moor_p.$$\" \"$moor_p\"; then".to_owned(),
            format!(
                "export JAVA_TOOL_OPTIONS=\"${{JAVA_TOOL_OPTIONS:+$JAVA_TOOL_OPTIONS }}-Djavax.net.ssl.trustStore=$moor_ts -Djavax.net.ssl.trustStorePassword=changeit -Djavax.net.ssl.trustStoreType=PKCS12 -Dhttps.proxyHost=127.0.0.1 -Dhttps.proxyPort={box_port} -Dhttp.proxyHost=127.0.0.1 -Dhttp.proxyPort={box_port} -Dhttp.nonProxyHosts={non_proxy} -Dhttps.nonProxyHosts={non_proxy}\""
            ),
            "else rm -f \"$moor_ts.$$\" \"$moor_p.$$\"; fi".to_owned(),
            "fi".to_owned(),
        ],
    );
    lines
}

#[cfg(test)]
mod tests;
