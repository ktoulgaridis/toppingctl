//! Package-manager packaging (#22): `scripts/package.sh render` fills the AUR
//! PKGBUILD/.SRCINFO and the Homebrew formula from a release's `SHA256SUMS`,
//! `scripts/package.sh apt-repo` stays a no-op for debs it already has, the
//! cargo-deb metadata ships the right files, and the `packages` workflow is
//! triggered and scoped the way the spec asks. Plain-text checks: no YAML or
//! TOML parser dependency.
#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const URL: &str = "https://github.com/ktoulgaridis/toppingctl";
const TARGETS: [&str; 4] = [
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
    "x86_64-unknown-linux-gnu",
    "aarch64-unknown-linux-gnu",
];

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn read(rel: &str) -> String {
    fs::read_to_string(root().join(rel)).unwrap_or_else(|e| panic!("{rel} must exist: {e}"))
}

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("packaging-{name}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn run(args: &[&str], envs: &[(&str, &str)]) -> Output {
    Command::new("bash")
        .arg(root().join("scripts/package.sh"))
        .args(args)
        .envs(envs.iter().copied())
        .output()
        .expect("bash runs")
}

fn ok(args: &[&str]) -> Output {
    let out = run(args, &[]);
    assert!(
        out.status.success(),
        "package.sh {args:?} failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

/// A distinct fake sha256 per file: the hex digit repeated 64 times.
fn fake(digit: char) -> String {
    std::iter::repeat_n(digit, 64).collect()
}

fn hash_for(target: &str) -> String {
    let i = TARGETS.iter().position(|t| *t == target).unwrap();
    fake(['a', 'b', 'c', 'd'][i])
}

fn src_hash() -> String {
    fake('e')
}

/// A SHA256SUMS for `tag`: the four archives, two debs, and the source tarball
/// the packages workflow adds (GitHub's tag archive, saved under this name).
fn sample_sums(dir: &Path, tag: &str) -> PathBuf {
    let ver = tag.trim_start_matches('v');
    let mut text = String::new();
    for t in TARGETS {
        text += &format!("{}  toppingctl-{tag}-{t}.tar.gz\n", hash_for(t));
    }
    text += &format!("{}  toppingctl_{ver}-1_amd64.deb\n", fake('1'));
    text += &format!("{}  toppingctl_{ver}-1_arm64.deb\n", fake('2'));
    text += &format!("{}  toppingctl-{tag}-source.tar.gz\n", src_hash());
    let path = dir.join("SHA256SUMS");
    fs::write(&path, text).unwrap();
    path
}

fn render(kind: &str, tag: &str, sums: &Path, out: &Path) {
    ok(&[
        "render",
        kind,
        tag,
        sums.to_str().unwrap(),
        out.to_str().unwrap(),
    ]);
}

fn has_line(text: &str, want: &str) -> bool {
    text.lines().any(|l| l == want)
}

fn assert_lines(file: &str, text: &str, wants: &[String]) {
    for w in wants {
        assert!(has_line(text, w), "{file} needs the line {w:?}:\n{text}");
    }
}

/// What every Linux package must install: the binary, the udev rule at the
/// system path, and both licences.
fn assert_installs_linux_files(file: &str, pkgbuild: &str) {
    for want in [
        "/usr/bin/toppingctl",
        "/usr/lib/udev/rules.d/70-toppingctl.rules",
        "packaging/linux/70-toppingctl.rules",
        "LICENSE-MIT",
        "LICENSE-APACHE",
        "/usr/share/licenses/",
    ] {
        assert!(pkgbuild.contains(want), "{file} must install {want}");
    }
}

// ---- AUR -------------------------------------------------------------------

#[test]
fn render_aur_bin_carries_version_urls_and_sha256() {
    let dir = scratch("aur-bin");
    let sums = sample_sums(&dir, "v9.9.9");
    let out = dir.join("out");
    render("aur", "v9.9.9", &sums, &out);

    let pkgbuild = fs::read_to_string(out.join("toppingctl-bin/PKGBUILD")).unwrap();
    assert_lines(
        "toppingctl-bin/PKGBUILD",
        &pkgbuild,
        &[
            "pkgname=toppingctl-bin".into(),
            "pkgver=9.9.9".into(),
            "pkgrel=1".into(),
            "arch=('x86_64' 'aarch64')".into(),
            format!("url=\"{URL}\""),
            format!(
                "sha256sums_x86_64=('{}')",
                hash_for("x86_64-unknown-linux-gnu")
            ),
            format!(
                "sha256sums_aarch64=('{}')",
                hash_for("aarch64-unknown-linux-gnu")
            ),
        ],
    );
    for arch in ["x86_64", "aarch64"] {
        let asset = format!("toppingctl-v${{pkgver}}-{arch}-unknown-linux-gnu.tar.gz");
        assert!(
            pkgbuild.contains(&format!("${{url}}/releases/download/v${{pkgver}}/{asset}")),
            "PKGBUILD source_{arch} must point at the release asset"
        );
    }
    assert_installs_linux_files("toppingctl-bin/PKGBUILD", &pkgbuild);

    let srcinfo = fs::read_to_string(out.join("toppingctl-bin/.SRCINFO")).unwrap();
    let mut wants = vec![
        "pkgbase = toppingctl-bin".to_string(),
        "\tpkgver = 9.9.9".into(),
        "\tpkgrel = 1".into(),
        "pkgname = toppingctl-bin".into(),
    ];
    for arch in ["x86_64", "aarch64"] {
        let asset = format!("toppingctl-v9.9.9-{arch}-unknown-linux-gnu.tar.gz");
        wants.push(format!(
            "\tsource_{arch} = {asset}::{URL}/releases/download/v9.9.9/{asset}"
        ));
        wants.push(format!(
            "\tsha256sums_{arch} = {}",
            hash_for(&format!("{arch}-unknown-linux-gnu"))
        ));
    }
    assert_lines("toppingctl-bin/.SRCINFO", &srcinfo, &wants);
    assert!(
        !srcinfo.contains("0.1.0"),
        "no stale version left:\n{srcinfo}"
    );
}

#[test]
fn render_aur_source_carries_version_url_and_sha256() {
    let dir = scratch("aur-src");
    let sums = sample_sums(&dir, "v9.9.9");
    let out = dir.join("out");
    render("aur", "v9.9.9", &sums, &out);

    let pkgbuild = fs::read_to_string(out.join("toppingctl/PKGBUILD")).unwrap();
    assert_lines(
        "toppingctl/PKGBUILD",
        &pkgbuild,
        &[
            "pkgname=toppingctl".into(),
            "pkgver=9.9.9".into(),
            "pkgrel=1".into(),
            format!("url=\"{URL}\""),
            format!("sha256sums=('{}')", src_hash()),
        ],
    );
    assert!(pkgbuild.contains("${url}/archive/refs/tags/v${pkgver}.tar.gz"));
    assert!(pkgbuild.contains("cargo build"), "builds from source");
    assert!(pkgbuild.contains("--locked") || pkgbuild.contains("--frozen"));
    assert_installs_linux_files("toppingctl/PKGBUILD", &pkgbuild);

    let srcinfo = fs::read_to_string(out.join("toppingctl/.SRCINFO")).unwrap();
    assert_lines(
        "toppingctl/.SRCINFO",
        &srcinfo,
        &[
            "pkgbase = toppingctl".into(),
            "\tpkgver = 9.9.9".into(),
            "\tmakedepends = cargo".into(),
            format!("\tsource = toppingctl-9.9.9.tar.gz::{URL}/archive/refs/tags/v9.9.9.tar.gz"),
            format!("\tsha256sums = {}", src_hash()),
        ],
    );
    assert!(!srcinfo.contains("0.1.0"), "{srcinfo}");
}

/// The in-repo PKGBUILDs are installable as they are (`makepkg -si` in
/// packaging/aur/<pkg>): they are the v0.1.0 render, so rendering v0.1.0's
/// real SHA256SUMS over them changes nothing.
#[test]
fn in_repo_aur_files_are_the_render_of_the_current_release() {
    let dir = scratch("aur-fixpoint");
    let sums = dir.join("SHA256SUMS");
    fs::write(
        &sums,
        "\
61645ef7479f4204a560f09599f435c238d142e2eadfddd9d27d0dc594bf6a5c  toppingctl-v0.1.0-aarch64-apple-darwin.tar.gz
f99a03eefdb34fab846ac68592c466de9dd3803d79b05e3cc8b2e08fceb1e7fa  toppingctl-v0.1.0-aarch64-unknown-linux-gnu.tar.gz
91d46802be91715f8f7f44af58d65898c47e9c66296e332963e64a6101a0f70e  toppingctl-v0.1.0-x86_64-apple-darwin.tar.gz
5a92c61aad6460acd67e3edb8a9a272ddad607145a4c58e5cd333c2bd6a01506  toppingctl-v0.1.0-x86_64-unknown-linux-gnu.tar.gz
70d96fe390261f25eae1ff03b4561a5282ff7d37eac6b55a1b5d23e19aa40a5a  toppingctl-v0.1.0-source.tar.gz
",
    )
    .unwrap();
    let out = dir.join("out");
    render("aur", "v0.1.0", &sums, &out);
    for pkg in ["toppingctl-bin", "toppingctl"] {
        for file in ["PKGBUILD", ".SRCINFO"] {
            let rel = format!("packaging/aur/{pkg}/{file}");
            assert_eq!(
                fs::read_to_string(out.join(pkg).join(file)).unwrap(),
                read(&rel),
                "{rel} must equal its v0.1.0 render"
            );
        }
    }
}

// ---- Homebrew --------------------------------------------------------------

#[test]
fn render_brew_formula_carries_version_urls_and_sha256() {
    let dir = scratch("brew");
    let sums = sample_sums(&dir, "v9.9.9");
    let out = dir.join("out");
    render("brew", "v9.9.9", &sums, &out);

    let rb = fs::read_to_string(out.join("toppingctl.rb")).unwrap();
    assert!(rb.contains("class Toppingctl < Formula"), "{rb}");
    assert!(has_line(&rb, "  version \"9.9.9\""), "{rb}");
    assert!(rb.contains("on_macos") && rb.contains("on_linux"));
    assert!(rb.contains("on_arm") && rb.contains("on_intel"));
    assert!(!rb.contains('@'), "no placeholder left:\n{rb}");
    assert!(
        rb.contains("--version"),
        "the formula's test runs --version"
    );

    // Each archive's url line is followed by its own sha256 line.
    let lines: Vec<&str> = rb.lines().map(str::trim).collect();
    for t in TARGETS {
        let url = format!("url \"{URL}/releases/download/v9.9.9/toppingctl-v9.9.9-{t}.tar.gz\"");
        let i = lines
            .iter()
            .position(|l| *l == url)
            .unwrap_or_else(|| panic!("formula needs {url}:\n{rb}"));
        assert_eq!(lines[i + 1], format!("sha256 \"{}\"", hash_for(t)), "{t}");
    }
}

#[test]
fn render_rejects_a_missing_sha256_entry() {
    let dir = scratch("missing-entry");
    let sums = sample_sums(&dir, "v9.9.9");
    let text = fs::read_to_string(&sums).unwrap();
    let cut: String = text
        .lines()
        .filter(|l| !l.contains("aarch64-unknown-linux-gnu"))
        .map(|l| format!("{l}\n"))
        .collect();
    fs::write(&sums, cut).unwrap();
    for kind in ["aur", "brew"] {
        let out = run(
            &[
                "render",
                kind,
                "v9.9.9",
                sums.to_str().unwrap(),
                dir.join(kind).to_str().unwrap(),
            ],
            &[],
        );
        assert!(
            !out.status.success(),
            "render {kind} must fail without a sha"
        );
    }
}

#[test]
fn render_rejects_a_tag_that_is_not_a_plain_version() {
    let dir = scratch("bad-tag");
    for tag in ["1.2.3", "v1.2.3-rc1", "v1.2", "v1.2.3/../x"] {
        let sums = sample_sums(&dir, "v9.9.9");
        let out = run(
            &[
                "render",
                "aur",
                tag,
                sums.to_str().unwrap(),
                dir.join("out").to_str().unwrap(),
            ],
            &[],
        );
        assert!(!out.status.success(), "tag {tag:?} must be rejected");
    }
}

// ---- deb -------------------------------------------------------------------

/// The `[package.metadata.deb]` table, as text.
fn deb_metadata() -> String {
    let toml = read("Cargo.toml");
    let start = toml
        .find("[package.metadata.deb]")
        .expect("Cargo.toml needs [package.metadata.deb] for cargo-deb");
    let rest = &toml[start + 1..];
    let end = rest.find("\n[").map_or(rest.len(), |i| i + 1);
    rest[..end].to_string()
}

#[test]
fn cargo_deb_metadata_ships_binary_udev_rule_and_licences() {
    let deb = deb_metadata();
    for want in [
        r#"["target/release/toppingctl", "usr/bin/", "755"]"#,
        r#"["packaging/linux/70-toppingctl.rules", "usr/lib/udev/rules.d/70-toppingctl.rules", "644"]"#,
        r#"["LICENSE-MIT", "usr/share/doc/toppingctl/", "644"]"#,
        r#"["LICENSE-APACHE", "usr/share/doc/toppingctl/", "644"]"#,
    ] {
        assert!(deb.contains(want), "deb assets need {want}:\n{deb}");
    }
    assert!(deb.contains("maintainer = "), "{deb}");
    assert!(deb.contains("depends = \"$auto\""), "{deb}");
    assert!(
        !read("Cargo.toml").contains("cargo-deb"),
        "cargo-deb is a CI tool, not a dependency"
    );
}

#[test]
fn sha256sums_covers_debs_too() {
    let dir = scratch("sums-deb");
    fs::write(
        dir.join("toppingctl-v9.9.9-x86_64-unknown-linux-gnu.tar.gz"),
        "a",
    )
    .unwrap();
    fs::write(dir.join("toppingctl_9.9.9-1_amd64.deb"), "b").unwrap();
    ok(&["sums", dir.to_str().unwrap()]);
    let sums = fs::read_to_string(dir.join("SHA256SUMS")).unwrap();
    assert!(sums.contains("  toppingctl_9.9.9-1_amd64.deb"), "{sums}");
    assert!(sums.contains("  toppingctl-v9.9.9-x86_64-unknown-linux-gnu.tar.gz"));
}

#[test]
fn release_workflow_builds_and_attaches_debs() {
    let text = read(".github/workflows/release.yml");
    assert!(
        text.contains("cargo install cargo-deb"),
        "cargo-deb runs in CI"
    );
    assert!(text.contains("--locked"), "pinned install");
    assert!(
        text.contains("cargo deb --no-build"),
        "packages the built binary"
    );
    assert!(
        text.contains("dist/*.deb"),
        "debs are uploaded and attached"
    );
    assert_eq!(text.matches("contents: write").count(), 1);
}

// ---- apt repo --------------------------------------------------------------

/// A repo that already has this exact deb in its pool: nothing to do, nothing
/// written, no signing key needed.
#[test]
fn apt_repo_is_a_no_op_for_a_deb_it_already_has() {
    let dir = scratch("apt-noop");
    let deb = dir.join("toppingctl_9.9.9-1_amd64.deb");
    fs::write(&deb, "deb bytes").unwrap();
    let repo = dir.join("repo");
    let pool = repo.join("pool/main/t/toppingctl");
    fs::create_dir_all(&pool).unwrap();
    fs::write(pool.join("toppingctl_9.9.9-1_amd64.deb"), "deb bytes").unwrap();

    let out = run(
        &["apt-repo", repo.to_str().unwrap(), deb.to_str().unwrap()],
        &[("APT_SIGNING_KEY", "")],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("unchanged"));
    assert!(!repo.join("dists").exists(), "nothing regenerated");
}

/// A published version never changes: a different deb under the same name
/// fails instead of overwriting.
#[test]
fn apt_repo_refuses_to_replace_a_published_deb() {
    let dir = scratch("apt-conflict");
    let deb = dir.join("toppingctl_9.9.9-1_amd64.deb");
    fs::write(&deb, "new bytes").unwrap();
    let repo = dir.join("repo");
    let pool = repo.join("pool/main/t/toppingctl");
    fs::create_dir_all(&pool).unwrap();
    fs::write(pool.join("toppingctl_9.9.9-1_amd64.deb"), "old bytes").unwrap();

    let out = run(
        &["apt-repo", repo.to_str().unwrap(), deb.to_str().unwrap()],
        &[("APT_SIGNING_KEY", "ABCD")],
    );
    assert!(!out.status.success());
    assert_eq!(
        fs::read_to_string(pool.join("toppingctl_9.9.9-1_amd64.deb")).unwrap(),
        "old bytes"
    );
}

// ---- the packages workflow -------------------------------------------------

/// Top-level `key:` blocks of a workflow: the text from `key:` to the next
/// unindented line.
fn top_block<'a>(text: &'a str, key: &str) -> &'a str {
    let start = text
        .find(&format!("\n{key}:"))
        .unwrap_or_else(|| panic!("workflow needs a top-level `{key}:`"))
        + 1;
    let rest = &text[start..];
    // The first later line that starts in column 0 (blank and comment lines
    // don't end a block).
    let end = rest
        .match_indices('\n')
        .map(|(i, _)| i + 1)
        .find(|&i| i < rest.len() && !rest[i..].starts_with([' ', '\n', '#']))
        .unwrap_or(rest.len());
    rest[..end].trim_end()
}

/// The jobs of a workflow: (name, body).
fn jobs(text: &str) -> Vec<(String, String)> {
    let block = top_block(text, "jobs");
    let mut out: Vec<(String, String)> = Vec::new();
    for line in block.lines().skip(1) {
        let is_job = line.starts_with("  ")
            && !line.starts_with("   ")
            && line.trim_end().ends_with(':')
            && !line.trim_start().starts_with('#');
        if is_job {
            out.push((line.trim().trim_end_matches(':').to_string(), String::new()));
        } else if let Some((_, body)) = out.last_mut() {
            body.push_str(line);
            body.push('\n');
        }
    }
    out
}

/// The steps of a job body, each as its text.
fn steps(body: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in body.lines() {
        if line.starts_with("      - ") {
            out.push(String::new());
        }
        if let Some(step) = out.last_mut() {
            step.push_str(line);
            step.push('\n');
        }
    }
    out
}

#[test]
fn packages_workflow_runs_on_release_published_not_on_tags() {
    let text = read(".github/workflows/packages.yml");
    let on = top_block(&text, "on");
    assert!(on.contains("release:"), "{on}");
    assert!(on.contains("types: [published]"), "{on}");
    assert!(!on.contains("push:") && !on.contains("tags:"), "{on}");
    assert!(!on.contains("pull_request"), "{on}");
    assert!(
        text.contains("github.event.release.prerelease"),
        "prereleases are not published to package managers"
    );
}

#[test]
fn packages_workflow_is_least_privilege() {
    let text = read(".github/workflows/packages.yml");
    let perms = top_block(&text, "permissions");
    assert_eq!(perms.trim(), "permissions:\n  contents: read", "{perms}");
    assert!(!text.contains("contents: write"));
    assert!(!text.contains("write-all"));

    // Every secret is read only in a job that runs in the `release` environment,
    // and no step reads more than one secret.
    const SECRETS: [&str; 3] = ["AUR_SSH_KEY", "PACKAGES_TOKEN", "APT_GPG_PRIVATE_KEY"];
    for secret in SECRETS {
        let r = format!("secrets.{secret}");
        assert!(text.contains(&r), "packages.yml should use {secret}");
        for (job, body) in jobs(&text).into_iter().filter(|(_, b)| b.contains(&r)) {
            assert!(
                body.contains("environment: release"),
                "job {job} reads {secret}, so it must run in the release environment"
            );
        }
    }
    assert_eq!(
        text.matches("secrets.AUR_SSH_KEY").count(),
        1,
        "the AUR key is read once"
    );
    assert_eq!(
        text.matches("secrets.APT_GPG_PRIVATE_KEY").count(),
        1,
        "the signing key is read once"
    );
    for (job, body) in jobs(&text) {
        for step in steps(&body) {
            let n = SECRETS
                .iter()
                .filter(|s| step.contains(&format!("secrets.{s}")))
                .count();
            assert!(n <= 1, "a step in {job} reads {n} secrets:\n{step}");
        }
        assert!(
            !body
                .lines()
                .take_while(|l| !l.trim_start().starts_with("steps:"))
                .any(|l| l.contains("secrets.")),
            "job {job}: secrets go in step env, not job env"
        );
    }
    assert!(
        !text.contains("secrets: inherit") && !text.contains("toJSON(secrets)"),
        "no blanket secret access"
    );
    for (job, body) in jobs(&text) {
        if !body.contains("secrets.") {
            assert!(
                !body.contains("environment: release"),
                "job {job} needs no secret, so no release environment"
            );
        }
    }
}

#[test]
fn packages_workflow_skips_the_aur_push_without_a_key() {
    let text = read(".github/workflows/packages.yml");
    let (_, aur) = jobs(&text)
        .into_iter()
        .find(|(_, b)| b.contains("secrets.AUR_SSH_KEY"))
        .unwrap();
    assert!(aur.contains("-z \"$AUR_SSH_KEY\""), "{aur}");
    assert!(aur.contains("::notice::"), "a skipped push leaves a notice");
    // Pinned AUR host key (from aur.archlinux.org), not trust-on-first-use.
    assert!(aur.contains(
        "aur.archlinux.org ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIEuBKrPzbawxA/k2g6NcyV5jmqwJ2s+zpgZGZ7tpLIcN"
    ));
    assert!(!aur.contains("ssh-keyscan") && !aur.contains("StrictHostKeyChecking=no"));
}

#[test]
fn packages_workflow_checks_every_install_after_publishing() {
    let text = read(".github/workflows/packages.yml");
    for want in [
        "archlinux",
        "makepkg -si",
        "debian:stable",
        "ubuntu:24.04",
        "signed-by=",
        "apt-get install -y toppingctl",
        "macos-latest",
        "brew install ktoulgaridis/tap/toppingctl",
        "toppingctl --version",
        "/usr/lib/udev/rules.d/70-toppingctl.rules",
    ] {
        assert!(text.contains(want), "packages.yml should contain {want:?}");
    }
    assert!(
        !text.contains("trusted=yes"),
        "apt must verify the signature"
    );
    assert!(!text.contains("allow-unauthenticated"));
}

#[test]
fn packages_workflow_publishes_idempotently() {
    let text = read(".github/workflows/packages.yml");
    // Commits only when something changed; the apt repo skips known debs.
    assert!(
        text.contains("git diff --cached --quiet"),
        "no empty commits"
    );
    assert!(text.contains("scripts/package.sh render aur"));
    assert!(text.contains("scripts/package.sh render brew"));
    assert!(text.contains("scripts/package.sh apt-repo"));
    assert!(text.contains("concurrency:"));
}

// ---- docs ------------------------------------------------------------------

#[test]
fn packaging_readme_documents_the_setup() {
    let text = read("packaging/README.md");
    for want in [
        "AUR_SSH_KEY",
        "PACKAGES_TOKEN",
        "APT_GPG_PRIVATE_KEY",
        "environment",
        "ktoulgaridis/homebrew-tap",
        "ktoulgaridis/apt",
        "GitHub Pages",
        "gpg",
        "makepkg -si",
        "packaging/aur/",
    ] {
        assert!(
            text.contains(want),
            "packaging/README.md should mention {want:?}"
        );
    }
}

#[test]
fn readme_install_lists_the_package_managers() {
    let text = read("README.md");
    for want in [
        "toppingctl-bin",
        "brew install ktoulgaridis/tap/toppingctl",
        "signed-by=",
        "apt-get install toppingctl",
    ] {
        assert!(text.contains(want), "README should mention {want:?}");
    }
}
