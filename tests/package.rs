//! Release packaging (#31): runs `scripts/package.sh` on a stand-in binary and checks
//! what the archives and `SHA256SUMS` contain, plus plain-text checks that
//! `release.yml` packages through the script.
#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const TAG: &str = "v9.9.9-test";

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// A fresh scratch directory under Cargo's per-test tmp dir.
fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("package-{name}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn package(args: &[&str]) -> std::process::Output {
    let out = Command::new("bash")
        .arg(root().join("scripts/package.sh"))
        .args(args)
        .output()
        .expect("bash runs");
    assert!(
        out.status.success(),
        "package.sh {args:?} failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

/// Packages a stand-in `toppingctl` for `target`; returns the archive's member list.
fn archive(target: &str, dir: &Path) -> Vec<String> {
    let bin = dir.join("bin");
    fs::create_dir_all(&bin).unwrap();
    fs::write(bin.join("toppingctl"), "#!/bin/sh\n").unwrap();
    // Dev tools sit next to it in target/<t>/release and must stay out.
    fs::write(bin.join("hidsend"), "").unwrap();
    fs::write(bin.join("hidsniff"), "").unwrap();
    let dist = dir.join("dist");
    package(&[
        "archive",
        TAG,
        target,
        bin.to_str().unwrap(),
        dist.to_str().unwrap(),
    ]);
    let tarball = dist.join(format!("toppingctl-{TAG}-{target}.tar.gz"));
    assert!(tarball.is_file(), "{} must exist", tarball.display());
    let out = Command::new("tar")
        .arg("-tzf")
        .arg(&tarball)
        .output()
        .unwrap();
    assert!(out.status.success());
    let mut names: Vec<String> = String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(|l| l.trim_end_matches('/').to_string())
        .collect();
    names.sort();
    names
}

fn expected(target: &str, linux: bool) -> Vec<String> {
    let top = format!("toppingctl-{TAG}-{target}");
    let mut files = vec!["toppingctl", "README.md", "LICENSE-MIT", "LICENSE-APACHE"];
    if root().join("CHANGELOG.md").is_file() {
        files.push("CHANGELOG.md");
    }
    if linux {
        files.extend([
            "packaging",
            "packaging/linux",
            "packaging/linux/70-toppingctl.rules",
            "packaging/linux/README.md",
        ]);
    }
    let mut names: Vec<String> = files.iter().map(|f| format!("{top}/{f}")).collect();
    names.push(top);
    names.sort();
    names
}

#[test]
fn linux_archive_has_binary_udev_rule_docs_and_licences() {
    let target = "x86_64-unknown-linux-gnu";
    let names = archive(target, &scratch("linux"));
    assert_eq!(names, expected(target, true));
}

#[test]
fn macos_archive_has_binary_docs_and_licences_without_udev_rule() {
    let target = "aarch64-apple-darwin";
    let names = archive(target, &scratch("macos"));
    assert_eq!(names, expected(target, false));
}

#[test]
fn sha256sums_covers_every_archive_and_verifies() {
    let dir = scratch("sums");
    archive("x86_64-unknown-linux-gnu", &dir);
    archive("aarch64-apple-darwin", &dir);
    let dist = dir.join("dist");
    package(&["sums", dist.to_str().unwrap()]);

    let sums = fs::read_to_string(dist.join("SHA256SUMS")).expect("SHA256SUMS written");
    let mut files: Vec<&str> = sums
        .lines()
        .map(|l| {
            let (hash, name) = l.split_once("  ").expect("`<hash>  <name>` lines");
            assert_eq!(hash.len(), 64, "sha256 hex: {l}");
            assert!(hash.bytes().all(|b| b.is_ascii_hexdigit()), "{l}");
            name
        })
        .collect();
    files.sort();
    assert_eq!(
        files,
        [
            format!("toppingctl-{TAG}-aarch64-apple-darwin.tar.gz"),
            format!("toppingctl-{TAG}-x86_64-unknown-linux-gnu.tar.gz"),
        ]
    );

    // The file checks out with the standard tool (what a user runs).
    let check = Command::new("bash")
        .arg("-c")
        .arg("if command -v sha256sum >/dev/null; then sha256sum -c SHA256SUMS; else shasum -a 256 -c SHA256SUMS; fi")
        .current_dir(&dist)
        .output()
        .unwrap();
    assert!(
        check.status.success(),
        "SHA256SUMS must verify:\n{}",
        String::from_utf8_lossy(&check.stdout)
    );
}

#[test]
fn package_rejects_a_missing_binary() {
    let dir = scratch("missing");
    let out = Command::new("bash")
        .arg(root().join("scripts/package.sh"))
        .args([
            "archive",
            TAG,
            "x86_64-unknown-linux-gnu",
            dir.join("nope").to_str().unwrap(),
            dir.join("dist").to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(!out.status.success(), "a missing binary must fail the job");
}

#[test]
fn release_workflow_packages_through_the_script() {
    let text = fs::read_to_string(root().join(".github/workflows/release.yml")).unwrap();
    assert!(
        text.contains("scripts/package.sh archive"),
        "release.yml must build archives with scripts/package.sh"
    );
    assert!(
        text.contains("scripts/package.sh sums"),
        "release.yml must write SHA256SUMS with scripts/package.sh"
    );
    assert!(text.contains("SHA256SUMS"), "SHA256SUMS must be uploaded");
    assert!(
        !text.contains("hidsend") && !text.contains("hidsniff"),
        "dev tools stay out of user archives"
    );
    // Least privilege: only the one publishing job may write.
    assert_eq!(
        text.matches("contents: write").count(),
        1,
        "exactly one job holds contents: write"
    );
}
