//! `cargo xtask ci` — the one entry point CI (or you) runs, so a workflow file
//! is just `cargo xtask ci`.

use std::process::Command;

use anyhow::{bail, Result};

/// The ordered steps (each is a full command line). Pure so it can be tested.
pub fn plan(packages: &[String], skip_tests: bool) -> Vec<Vec<String>> {
    let scope: Vec<String> = if packages.is_empty() {
        vec!["--workspace".into()]
    } else {
        packages
            .iter()
            .flat_map(|p| ["-p".to_owned(), p.clone()])
            .collect()
    };
    let cargo = |args: &[&str]| -> Vec<String> {
        std::iter::once("cargo".to_owned())
            .chain(args.iter().map(|s| (*s).to_owned()))
            .collect()
    };
    let mut steps = vec![
        cargo(&["fmt", "--all", "--", "--check"]),
        cargo(&["xtask", "gen-mls-storage", "--check"]),
        cargo(&["xtask", "audit-pure"]),
    ];
    let mut clippy = cargo(&["clippy"]);
    clippy.extend(scope.clone());
    clippy.extend(["--all-targets", "--", "-D", "warnings"].map(String::from));
    steps.push(clippy);
    if !skip_tests {
        let mut t = cargo(&["test"]);
        t.extend(scope);
        steps.push(t);
    }
    steps
}

pub fn run(packages: &[String], skip_tests: bool) -> Result<()> {
    for step in plan(packages, skip_tests) {
        println!("\n$ {}", step.join(" "));
        let st = Command::new(&step[0]).args(&step[1..]).status()?;
        if !st.success() {
            bail!("step failed: {}", step.join(" "));
        }
    }
    println!("\nci: all steps passed");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_plan_checks_format_generated_code_native_deps_lints_then_tests() {
        let p: Vec<String> = plan(&[], false).iter().map(|s| s.join(" ")).collect();
        assert_eq!(p[0], "cargo fmt --all -- --check");
        assert!(p.iter().any(|s| s == "cargo xtask gen-mls-storage --check"));
        assert!(p.iter().any(|s| s == "cargo xtask audit-pure"));
        assert!(p
            .iter()
            .any(|s| s == "cargo clippy --workspace --all-targets -- -D warnings"));
        assert_eq!(p.last().unwrap(), "cargo test --workspace");
    }

    #[test]
    fn package_scoping_and_skipping_tests_are_honoured() {
        let p: Vec<String> = plan(&["siar-daemon".into()], true)
            .iter()
            .map(|s| s.join(" "))
            .collect();
        assert!(p
            .iter()
            .any(|s| s == "cargo clippy -p siar-daemon --all-targets -- -D warnings"));
        assert!(!p.iter().any(|s| s.starts_with("cargo test")));
    }
}
