//! Plain-text checks on the Linux udev rule and its install docs.

use std::fs;
use std::path::Path;

const RULE: &str = "packaging/linux/70-toppingctl.rules";
const README: &str = "packaging/linux/README.md";

fn read(rel: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("{rel} must exist: {e}"))
}

/// The rule lines of a udev rules file: no blanks, no comments.
fn rule_lines(text: &str) -> Vec<&str> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect()
}

/// The comma-separated key/operator/value tokens of one rule line.
fn tokens(line: &str) -> Vec<&str> {
    line.split(',')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .collect()
}

#[test]
fn rule_is_seat_scoped() {
    let text = read(RULE);
    let lines = rule_lines(&text);
    assert_eq!(
        lines.len(),
        1,
        "{RULE}: expected exactly one rule, got {lines:?}"
    );
    let toks = tokens(lines[0]);
    for want in [
        r#"SUBSYSTEM=="hidraw""#,
        r#"ATTRS{idVendor}=="152a""#,
        r#"ATTRS{idProduct}=="8752""#,
        r#"TAG+="uaccess""#,
    ] {
        assert!(toks.contains(&want), "{RULE}: missing `{want}` in {toks:?}");
    }
    // Access comes only from the seat (uaccess): no mode bits, group or owner grant.
    for tok in &toks {
        for banned in ["MODE", "GROUP", "OWNER"] {
            assert!(
                !tok.starts_with(banned),
                "{RULE}: `{tok}` grants access beyond the logged-in seat user"
            );
        }
    }
}

#[test]
fn readme_covers_install_and_uninstall() {
    let text = read(README);
    for want in [
        "70-toppingctl.rules",
        "/etc/udev/rules.d/",
        "udevadm control --reload",
        "udevadm trigger",
        "replug",
        "Uninstall",
    ] {
        assert!(text.contains(want), "{README}: missing `{want}`");
    }
}
