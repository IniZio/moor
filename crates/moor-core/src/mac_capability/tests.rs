use super::*;

use MacCapability::{InterfaceBuilder, Toolchain, XcodeBuild};

fn render(sel: Selection) -> String {
    render_list(&expand(&selected(sel)))
}

#[test]
fn selected_sets() {
    assert_eq!(selected(Selection::default()), vec![Toolchain]);
    let xcode = Selection {
        xcode: true,
        ios_simulator: false,
    };
    assert_eq!(
        selected(xcode),
        vec![Toolchain, XcodeBuild, InterfaceBuilder]
    );
    let sim = Selection {
        xcode: false,
        ios_simulator: true,
    };
    assert_eq!(selected(sim), MacCapability::ALL.to_vec());
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
xcode-build pref pref:com.apple.dt.Xcode # Xcode reads its preferences
xcode-build pref pref:com.apple.dt.xcodebuild # xcodebuild reads its preferences
interface-builder pref pref:com.apple.ibtool # ibtool writes its preference
interface-builder read login-home:Library/Developer/CoreSimulator # actool and ibtool start AssetCatalogSimulatorAgent through CoreSimulator
interface-builder read login-home:Library/Developer/DVTDownloads # Metal toolchain download directory
";
    assert_eq!(
        render(Selection {
            xcode: true,
            ios_simulator: false
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
xcode-build pref pref:com.apple.dt.Xcode # Xcode reads its preferences
xcode-build pref pref:com.apple.dt.xcodebuild # xcodebuild reads its preferences
interface-builder pref pref:com.apple.ibtool # ibtool writes its preference
interface-builder read login-home:Library/Developer/CoreSimulator # actool and ibtool start AssetCatalogSimulatorAgent through CoreSimulator
interface-builder read login-home:Library/Developer/DVTDownloads # Metal toolchain download directory
ios-simulator write sim: # the simulator device set of the Box holds device state
ios-simulator read system:/Library/Developer/CoreSimulator # simulator runtimes are installed system-wide
ios-simulator read login-home:Library/Developer/CoreSimulator # CoreSimulator reads the logged-in user's device data
";
    let all = render(Selection {
        xcode: false,
        ios_simulator: true,
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
        };
        assert!(!text.contains("Library/Logs"), "{text}");
    }
}
