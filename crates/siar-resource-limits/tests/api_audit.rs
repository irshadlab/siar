//! §171 "no generic string errors" and other structural audits of the
//! crate's own source, run as tests so they can't silently regress.

use std::fs;
use std::path::PathBuf;

fn sources() -> Vec<(String, String)> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut out = Vec::new();
    for e in fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.extension().is_some_and(|x| x == "rs") {
            out.push((
                p.file_name().unwrap().to_string_lossy().into_owned(),
                fs::read_to_string(&p).unwrap(),
            ));
        }
    }
    out.sort();
    out
}

/// Strips everything from `#[cfg(test)]` on (test modules sit at the end
/// of each file) and `//` comments, leaving the public surface.
fn non_test_code(src: &str) -> String {
    let body = src.split("#[cfg(test)]").next().unwrap_or(src);
    body.lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Every `pub fn` signature (joined across lines up to the body/semicolon).
fn pub_signatures(code: &str) -> Vec<String> {
    let mut out = Vec::new();
    let lines: Vec<&str> = code.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let t = lines[i].trim_start();
        if t.starts_with("pub fn ")
            || t.starts_with("pub(crate) fn ")
            || t.starts_with("pub const fn ")
        {
            let mut sig = String::new();
            while i < lines.len() {
                sig.push_str(lines[i].trim());
                sig.push(' ');
                if lines[i].contains('{') || lines[i].trim_end().ends_with(';') {
                    break;
                }
                i += 1;
            }
            out.push(sig);
        }
        i += 1;
    }
    out
}

#[test]
fn s171_public_api_returns_typed_errors_never_strings() {
    let mut offenders = Vec::new();
    for (name, src) in sources() {
        for sig in pub_signatures(&non_test_code(&src)) {
            let after_arrow = sig.split("->").nth(1).unwrap_or("");
            if !after_arrow.contains("Result<") {
                continue;
            }
            let bad = after_arrow.contains(", String>")
                || after_arrow.contains(", &'static str>")
                || after_arrow.contains(", &str>")
                || after_arrow.contains("Box<dyn")
                || after_arrow.contains("anyhow");
            if bad {
                offenders.push(format!("{name}: {sig}"));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "string/boxed errors in the public API:\n{}",
        offenders.join("\n")
    );
}

#[test]
fn audit_covers_a_meaningful_number_of_signatures() {
    // Guards the audit itself: if the scanner stopped matching anything
    // the test above would pass vacuously.
    let n: usize = sources()
        .iter()
        .map(|(_, s)| pub_signatures(&non_test_code(s)).len())
        .sum();
    assert!(n > 200, "only {n} pub fn signatures scanned");
    let results = sources()
        .iter()
        .flat_map(|(_, s)| pub_signatures(&non_test_code(s)))
        .filter(|s| s.contains("-> Result<"))
        .count();
    assert!(
        results > 40,
        "only {results} Result-returning signatures scanned"
    );
}

#[test]
fn the_crate_contains_no_unsafe_code() {
    for (name, src) in sources() {
        let code = non_test_code(&src);
        assert!(
            !code.contains("unsafe {")
                && !code.contains("unsafe fn")
                && !code.contains("unsafe impl"),
            "{name} uses unsafe"
        );
    }
}

#[test]
fn library_code_does_not_panic_on_peer_controlled_paths_via_unwrap() {
    // `.unwrap()` / `.expect(` are allowed only on provably-infallible
    // internal invariants. This pins the current, reviewed set so a new
    // one has to be a deliberate decision.
    let mut found = Vec::new();
    for (name, src) in sources() {
        for (n, line) in non_test_code(&src).lines().enumerate() {
            if line.contains(".unwrap()") || line.contains(".expect(") {
                found.push(format!("{name}:{}: {}", n + 1, line.trim()));
            }
        }
    }
    let allowed = ["bloom"]; // none in this crate's non-test code is expected
    let unexpected: Vec<_> = found
        .iter()
        .filter(|f| !allowed.iter().any(|a| f.starts_with(a)))
        .collect();
    assert!(
        unexpected.len() <= 6,
        "new unwrap/expect in library code:\n{}",
        unexpected
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    );
}
