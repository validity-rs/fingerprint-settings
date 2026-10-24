//! The packages say the same version as the crate: the metainfo's latest
//! release, the Debian changelog, the Fedora spec and the manual page. (The
//! PKGBUILD and APKBUILD are written from Cargo.toml's version by the build.)

use std::path::Path;

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn read(path: &str) -> String {
    std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(path))
        .unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// The text between `start` and the next `end` after it, in `text`.
fn between<'a>(text: &'a str, start: &str, end: &str) -> Option<&'a str> {
    let from = text.find(start)? + start.len();
    let len = text[from..].find(end)?;
    Some(&text[from..from + len])
}

#[test]
fn metainfo_latest_release_is_the_crate_version() {
    let m = read("packaging/common/io.github.validity_rs.FingerprintSettings.metainfo.xml");
    let releases = between(&m, "<releases>", "</releases>").expect("<releases>");
    let first = between(releases, "<release version=\"", "\"").expect("a <release>");
    assert_eq!(first, VERSION);
}

#[test]
fn debian_changelog_is_the_crate_version() {
    let c = read("packaging/debian/changelog");
    let first = c.lines().next().unwrap_or_default();
    assert!(
        first.starts_with(&format!("fingerprint-settings ({VERSION}-")),
        "packaging/debian/changelog starts with {first:?}"
    );
}

#[test]
fn fedora_spec_is_the_crate_version() {
    let s = read("packaging/fedora/fingerprint-settings.spec");
    let version = s
        .lines()
        .find_map(|l| l.strip_prefix("Version:"))
        .map(str::trim)
        .expect("Version: in the spec");
    assert_eq!(version, VERSION);
    let changelog = between(&s, "%changelog\n", "\n").expect("%changelog");
    assert!(
        changelog.ends_with(&format!(" - {VERSION}-1")),
        "the spec's last %changelog entry is {changelog:?}"
    );
}

#[test]
fn manual_page_is_the_crate_version() {
    let m = read("packaging/common/fingerprint-settings.1");
    let th = m.lines().find(|l| l.starts_with(".TH ")).expect(".TH");
    assert!(
        th.contains(&format!("\"fingerprint-settings {VERSION}\"")),
        "{th}"
    );
}
