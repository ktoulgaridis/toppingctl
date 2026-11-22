//! Plain-text checks on the GitHub Actions workflows (no YAML parser dependency).

use std::fs;
use std::path::Path;

const WORKFLOWS: [&str; 2] = [".github/workflows/ci.yml", ".github/workflows/release.yml"];

fn read(rel: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("{rel} must exist: {e}"))
}

fn is_sha(s: &str) -> bool {
    s.len() == 40
        && s.bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

/// The value of a `uses:` key on this line, if the line has one.
fn uses_value(line: &str) -> Option<&str> {
    let t = line.trim_start().trim_start_matches("- ").trim_start();
    let rest = t.strip_prefix("uses:")?;
    // Drop a trailing `# vX.Y.Z` comment and any quoting.
    let v = rest.split('#').next().unwrap_or("").trim();
    Some(v.trim_matches(|c| c == '"' || c == '\''))
}

#[test]
fn workflows_are_pinned() {
    for rel in WORKFLOWS {
        let text = read(rel);

        // Basic shape: a YAML workflow with triggers and jobs, indented with spaces.
        assert!(
            !text.contains('\t'),
            "{rel}: tabs are not valid YAML indentation"
        );
        assert!(
            text.lines().any(|l| l == "on:"),
            "{rel}: missing top-level `on:`"
        );
        assert!(
            text.lines().any(|l| l == "jobs:"),
            "{rel}: missing top-level `jobs:`"
        );
        assert!(
            text.lines().any(|l| l == "permissions:"),
            "{rel}: missing top-level `permissions:`"
        );

        assert!(
            !text.contains("pull_request_target"),
            "{rel}: pull_request_target is not allowed"
        );

        for (n, line) in text.lines().enumerate() {
            let Some(v) = uses_value(line) else { continue };
            // Local actions (./path) and docker:// refs are not third-party pins.
            if v.starts_with("./") || v.starts_with("docker://") {
                continue;
            }
            let (action, rev) = v
                .split_once('@')
                .unwrap_or_else(|| panic!("{rel}:{}: `uses: {v}` has no @ref", n + 1));
            assert!(!action.is_empty(), "{rel}:{}: empty action name", n + 1);
            assert!(
                is_sha(rev),
                "{rel}:{}: `uses: {v}` must be pinned to a full 40-hex commit SHA",
                n + 1
            );
        }
    }
}

#[test]
fn uses_value_parses_pins() {
    let sha = "fbc6f3992d24b796d5a048ff273f7fcc4a7b6c09";
    let line = format!("      - uses: actions/checkout@{sha} # v5.1.0");
    assert_eq!(
        uses_value(&line),
        Some(format!("actions/checkout@{sha}").as_str())
    );
    assert_eq!(
        uses_value("        uses: actions/checkout@v5"),
        Some("actions/checkout@v5")
    );
    assert_eq!(uses_value("      - run: cargo test"), None);
    assert!(is_sha(sha));
    assert!(!is_sha("v5"));
    assert!(!is_sha(&sha.to_uppercase()));
}
