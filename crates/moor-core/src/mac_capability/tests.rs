use super::*;

use MacCapability::{InterfaceBuilder, SwiftpmPlugins, Toolchain, XcodeBuild};

fn render(sel: Selection) -> String {
    render_list(&expand(&selected(sel)))
}

#[test]
fn selected_sets() {
    assert_eq!(selected(Selection::default()), vec![Toolchain]);
    let xcode = Selection {
        xcode: true,
        ios_simulator: false,
        swiftpm_plugins: false,
    };
    assert_eq!(
        selected(xcode),
        vec![Toolchain, XcodeBuild, InterfaceBuilder]
    );
    let sim = Selection {
        xcode: false,
        ios_simulator: true,
        swiftpm_plugins: false,
    };
    let mut no_plugins = MacCapability::ALL.to_vec();
    no_plugins.retain(|c| *c != SwiftpmPlugins);
    assert_eq!(selected(sim), no_plugins);
    let plugins = Selection {
        swiftpm_plugins: true,
        ..Selection::default()
    };
    assert_eq!(
        selected(plugins),
        vec![Toolchain, XcodeBuild, InterfaceBuilder, SwiftpmPlugins]
    );
}

#[test]
fn every_grant_has_one_line_reason_and_owner() {
    for cap in MacCapability::ALL {
        assert!(!cap.grants().is_empty(), "{} has no grants", cap.name());
        for g in cap.grants() {
            assert_eq!(g.capability, cap);
            assert!(!g.reason.trim().is_empty());
            assert!(!g.reason.contains('\n'));
        }
    }
}

#[test]
fn snapshot_toolchain() {
    let expected = "\
toolchain read developer-dir: # compilers and SDKs live in the active developer directory
toolchain read system:/Library/Developer # system-wide developer tools and command line tools
toolchain read system:/Library/Apple # Apple-installed support files the toolchain loads
toolchain read system:/var/select # xcode-select links resolve the active developer directory
toolchain write work: # the agent edits and builds in the working directory
toolchain write home: # tools keep per-user state in the Box home
toolchain write tmp: # tools write scratch files to the temporary directory
toolchain write cache: # tools write caches to the cache directory
toolchain read launch: # moor-written launch files are readable but not editable
";
    assert_eq!(render(Selection::default()), expected);
}

#[test]
fn snapshot_xcode() {
    let expected = "\
toolchain read developer-dir: # compilers and SDKs live in the active developer directory
toolchain read system:/Library/Developer # system-wide developer tools and command line tools
toolchain read system:/Library/Apple # Apple-installed support files the toolchain loads
toolchain read system:/var/select # xcode-select links resolve the active developer directory
toolchain write work: # the agent edits and builds in the working directory
toolchain write home: # tools keep per-user state in the Box home
toolchain write tmp: # tools write scratch files to the temporary directory
toolchain write cache: # tools write caches to the cache directory
toolchain read launch: # moor-written launch files are readable but not editable
xcode-build write home:Library/Developer/Xcode # DerivedData and module cache
xcode-build write home:Library/Caches # Xcode and clang write caches under the Box home
xcode-build pref pref:com.apple.dt.Xcode # Xcode reads and writes its preferences
xcode-build pref pref:com.apple.dt.xcodebuild # xcodebuild reads and writes its preferences
interface-builder pref pref:com.apple.ibtool # ibtool reads and writes its preferences
interface-builder read login-home:Library/Developer/CoreSimulator # actool and ibtool start AssetCatalogSimulatorAgent through CoreSimulator
interface-builder read login-home:Library/Developer/DVTDownloads # Metal toolchain download directory
";
    assert_eq!(
        render(Selection {
            xcode: true,
            ios_simulator: false,
            swiftpm_plugins: false,
        }),
        expected
    );
}

#[test]
fn snapshot_ios_simulator() {
    let expected = "\
toolchain read developer-dir: # compilers and SDKs live in the active developer directory
toolchain read system:/Library/Developer # system-wide developer tools and command line tools
toolchain read system:/Library/Apple # Apple-installed support files the toolchain loads
toolchain read system:/var/select # xcode-select links resolve the active developer directory
toolchain write work: # the agent edits and builds in the working directory
toolchain write home: # tools keep per-user state in the Box home
toolchain write tmp: # tools write scratch files to the temporary directory
toolchain write cache: # tools write caches to the cache directory
toolchain read launch: # moor-written launch files are readable but not editable
xcode-build write home:Library/Developer/Xcode # DerivedData and module cache
xcode-build write home:Library/Caches # Xcode and clang write caches under the Box home
xcode-build pref pref:com.apple.dt.Xcode # Xcode reads and writes its preferences
xcode-build pref pref:com.apple.dt.xcodebuild # xcodebuild reads and writes its preferences
interface-builder pref pref:com.apple.ibtool # ibtool reads and writes its preferences
interface-builder read login-home:Library/Developer/CoreSimulator # actool and ibtool start AssetCatalogSimulatorAgent through CoreSimulator
interface-builder read login-home:Library/Developer/DVTDownloads # Metal toolchain download directory
ios-simulator write sim: # the simulator device set of the Box holds device state
ios-simulator read system:/Library/Developer/CoreSimulator # simulator runtimes are installed system-wide
ios-simulator read login-home:Library/Developer/CoreSimulator # CoreSimulator reads the logged-in user's device data
";
    let all = render(Selection {
        xcode: false,
        ios_simulator: true,
        swiftpm_plugins: false,
    });
    assert_eq!(all, expected);
    assert!(!all.contains("(allow"));
}

#[test]
fn no_grant_names_library_logs() {
    for g in expand(&MacCapability::ALL) {
        let text = match g.grant {
            Grant::Read(p) | Grant::Write(p) => p.rel,
            Grant::PrefDomain(s) | Grant::MachLookup(s) => s,
            Grant::NestedSandboxExec => continue,
        };
        assert!(!text.contains("Library/Logs"), "{text}");
    }
}

#[test]
fn snapshot_swiftpm_plugins_adds_one_reasoned_grant() {
    let plain = render(Selection {
        xcode: true,
        ..Selection::default()
    });
    let with = render(Selection {
        swiftpm_plugins: true,
        ..Selection::default()
    });
    let expected = format!(
        "{plain}swiftpm-plugins exec nested-sandbox-exec # Xcode wraps plugin commands in sandbox-exec; any command run through sandbox-exec can leave the sandbox\n"
    );
    assert_eq!(with, expected);
    assert!(!plain.contains("nested-sandbox-exec"));
}

#[test]
fn host_toolchain_roots_add_one_read_each_under_toolchain() {
    let base = expand(&[Toolchain]);
    let got = expand_with(&[Toolchain], 2);
    assert_eq!(expand_with(&[Toolchain], 0), base);
    let (head, added) = got.split_at(base.len());
    assert_eq!(head, base.as_slice());
    assert_eq!(
        render_list(added),
        "toolchain read host-toolchain[0]: # owner-configured host toolchain\n\
         toolchain read host-toolchain[1]: # owner-configured host toolchain\n"
    );
}

#[test]
fn host_toolchain_roots_need_the_toolchain_capability() {
    assert_eq!(expand_with(&[XcodeBuild], 3), expand(&[XcodeBuild]));
}
