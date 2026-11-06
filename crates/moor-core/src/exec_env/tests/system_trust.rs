//! The prelude's system trust lines run under a real `sh` against a distro tree in a temp root.

use std::path::{Path, PathBuf};

use super::*;

const SUDO_WORKS: &str = "#!/bin/sh\nshift\nexec \"$@\"\n";
const SUDO_REFUSES: &str = "#!/bin/sh\nexit 1\n";
const SUDO_CANNOT_INSTALL: &str =
    "#!/bin/sh\nshift\n[ \"$1\" != install ] || exit 1\nexec \"$@\"\n";

const DISTRO_CA: &str = "-----BEGIN CERTIFICATE-----\nRElTVFJP\n-----END CERTIFICATE-----\n";
const CA_LINE: &str = "QUJD'REVG";

/// Where a distro keeps its bundle and the directory its own tool rebuilds that bundle from.
#[derive(Clone, Copy)]
struct Distro {
    bundle: &'static str,
    anchors: &'static str,
}

const DEBIAN: Distro = Distro {
    bundle: "etc/ssl/certs/ca-certificates.crt",
    anchors: "usr/local/share/ca-certificates",
};
const RHEL: Distro = Distro {
    bundle: "etc/pki/tls/certs/ca-bundle.crt",
    anchors: "etc/pki/ca-trust/source/anchors",
};

struct Root {
    tmp: tempfile::TempDir,
    distro: Distro,
}

impl Root {
    fn new(distro: Distro, sudo: &str) -> Self {
        let root = Self {
            tmp: tempfile::Builder::new()
                .prefix("moor-trust-")
                .tempdir()
                .unwrap(),
            distro,
        };
        for dir in ["home", "bin", distro.anchors] {
            std::fs::create_dir_all(root.at(dir)).unwrap();
        }
        std::fs::create_dir_all(root.bundle().parent().unwrap()).unwrap();
        std::fs::write(root.bundle(), DISTRO_CA).unwrap();
        root.sudo(sudo);
        root
    }

    fn at(&self, rel: &str) -> PathBuf {
        self.tmp.path().join(rel)
    }

    fn bundle(&self) -> PathBuf {
        self.at(self.distro.bundle)
    }

    fn anchor(&self) -> PathBuf {
        self.at(self.distro.anchors).join("moor-proxy.crt")
    }

    fn built_bundle(&self) -> PathBuf {
        self.at("home/.moor-proxy/ca-bundle.pem")
    }

    fn sudo(&self, body: &str) {
        moor_testkit::write_executable(&self.at("bin/sudo"), body).unwrap();
    }

    /// The prelude lines that write the CA and trust it, with every system path under this root,
    /// run under umask 077; returns the bundle the exports would name.
    fn prelude(&self) -> PathBuf {
        self.prelude_with(PEM)
    }

    fn prelude_with(&self, pem: &str) -> PathBuf {
        let got = build(&ctx(&[], Some(pem), None), Purpose::Run).unwrap();
        let at = |start: &str| got.prelude.iter().position(|l| l.starts_with(start));
        let lines = got
            .prelude
            .get(at("moor_ca=").unwrap()..=at("if [ -n \"$moor_sys\" ]").unwrap())
            .unwrap();
        let root = self.tmp.path().display();
        let script = lines
            .join("\n")
            .replace("/etc/ssl/", &format!("{root}/etc/ssl/"))
            .replace("/etc/pki/", &format!("{root}/etc/pki/"))
            .replace("/usr/local/share/", &format!("{root}/usr/local/share/"));
        let out = std::process::Command::new("sh")
            .arg("-c")
            .arg(format!("umask 077\n{script}\nprintf '%s' \"$moor_bundle\""))
            .env("HOME", self.at("home"))
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    self.at("bin").display(),
                    std::env::var("PATH").unwrap_or_default()
                ),
            )
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        PathBuf::from(String::from_utf8(out.stdout).unwrap())
    }

    /// What the distro's tool does when a package upgrade runs it: the bundle becomes the distro's
    /// own CAs plus each `*.crt` in the anchor directory, and nothing else it held survives.
    fn rebuild_bundle(&self) {
        let mut bundle = DISTRO_CA.to_owned();
        let mut anchors: Vec<_> = std::fs::read_dir(self.at(self.distro.anchors))
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|x| x == "crt"))
            .collect();
        anchors.sort();
        for anchor in anchors {
            bundle.push_str(&std::fs::read_to_string(anchor).unwrap());
        }
        std::fs::write(self.bundle(), bundle).unwrap();
    }
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap()
}

fn copies_of_ca(bundle: &Path) -> usize {
    read(bundle).lines().filter(|l| *l == CA_LINE).count()
}

#[test]
fn a_bundle_rebuilt_from_the_distros_directory_still_holds_the_ca() {
    for distro in [DEBIAN, RHEL] {
        let root = Root::new(distro, SUDO_WORKS);
        assert_eq!(root.prelude(), root.bundle());
        assert_eq!(copies_of_ca(&root.bundle()), 1);
        root.rebuild_bundle();
        assert_eq!(copies_of_ca(&root.bundle()), 1, "{}", distro.bundle);
        assert!(read(&root.bundle()).starts_with(DISTRO_CA));
    }
}

#[test]
fn the_anchor_is_world_readable_under_a_private_umask() {
    let root = Root::new(DEBIAN, SUDO_WORKS);
    root.prelude();
    let mode = std::fs::metadata(root.anchor()).unwrap();
    assert_eq!(
        std::os::unix::fs::PermissionsExt::mode(&mode.permissions()) & 0o777,
        0o644
    );
}

#[test]
fn without_sudo_the_system_stays_untouched_and_the_built_bundle_holds_the_ca() {
    for distro in [DEBIAN, RHEL] {
        let root = Root::new(distro, SUDO_REFUSES);
        assert_eq!(root.prelude(), root.built_bundle());
        assert_eq!(read(&root.bundle()), DISTRO_CA);
        assert!(!root.anchor().exists());
        assert_eq!(read(&root.built_bundle()), format!("{DISTRO_CA}{PEM}"));
    }
}

#[test]
fn a_second_prelude_changes_nothing() {
    for distro in [DEBIAN, RHEL] {
        let root = Root::new(distro, SUDO_WORKS);
        root.prelude();
        let (bundle, anchor) = (read(&root.bundle()), root.anchor());
        let inode = std::os::unix::fs::MetadataExt::ino(&std::fs::metadata(&anchor).unwrap());
        // A write through sudo would fail here, so an unneeded one surfaces as the built bundle.
        root.sudo("#!/bin/sh\nshift\n[ \"$1\" = true ] || exit 1\nexec \"$@\"\n");
        assert_eq!(root.prelude(), root.bundle());
        assert_eq!(read(&root.bundle()), bundle);
        assert_eq!(
            std::os::unix::fs::MetadataExt::ino(&std::fs::metadata(&anchor).unwrap()),
            inode
        );
    }
}

#[test]
fn a_new_ca_replaces_the_anchor_so_a_rebuild_holds_only_the_current_one() {
    let root = Root::new(DEBIAN, SUDO_WORKS);
    root.prelude();
    let next = "-----BEGIN CERTIFICATE-----\nTkVYVA==\n-----END CERTIFICATE-----\n";
    root.prelude_with(next);
    root.rebuild_bundle();
    assert_eq!(read(&root.bundle()), format!("{DISTRO_CA}{next}"));
}

#[test]
fn an_anchor_that_cannot_be_written_falls_back_to_the_built_bundle() {
    let root = Root::new(DEBIAN, SUDO_CANNOT_INSTALL);
    assert_eq!(root.prelude(), root.built_bundle());
    assert_eq!(copies_of_ca(&root.built_bundle()), 2);
}

#[test]
fn a_distro_with_no_anchor_directory_keeps_the_appended_bundle_and_gains_no_directory() {
    let root = Root::new(DEBIAN, SUDO_WORKS);
    std::fs::remove_dir(root.at(DEBIAN.anchors)).unwrap();
    assert_eq!(root.prelude(), root.bundle());
    assert_eq!(copies_of_ca(&root.bundle()), 1);
    assert!(!root.at(DEBIAN.anchors).exists());
}

#[test]
fn the_next_exec_repairs_a_bundle_rebuilt_without_the_anchor() {
    let root = Root::new(DEBIAN, SUDO_WORKS);
    root.prelude();
    std::fs::remove_file(root.anchor()).unwrap();
    root.rebuild_bundle();
    assert_eq!(copies_of_ca(&root.bundle()), 0);
    root.prelude();
    assert_eq!(copies_of_ca(&root.bundle()), 1);
}
