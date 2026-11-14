//! `cargo xtask audit-pure` — find C/C++ in the dependency graph.
//!
//! Ground truth for *what is built* is `cargo tree` (it applies feature
//! resolution exactly). `cargo metadata` lists extra edges for weak feature
//! dependencies (`dep?/feature`), so using it alone over-counts — our first
//! audit did exactly that and reported SQLite long after it was removed. The
//! metadata is therefore only used for per-package detail (build-dependencies,
//! `links`, active features), restricted to the packages `cargo tree` reports.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::process::Command;

use anyhow::{bail, Context, Result};
use cargo_metadata::{DependencyKind, MetadataCommand, PackageId};
use serde::Deserialize;

const POLICY: &str = include_str!("../audit-pure.ron");

#[derive(Debug, Deserialize)]
pub struct Policy {
    pub native_build_deps: Vec<String>,
    pub pure_links: Vec<(String, String)>,
    pub inactive_when: Vec<(String, String, String)>,
    pub exempt_roots: Vec<String>,
    pub accepted: Vec<(String, String)>,
    pub tracked: Vec<(String, String)>,
}

pub fn policy() -> Result<Policy> {
    ron::from_str(POLICY).context("parsing xtask/audit-pure.ron")
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Finding {
    pub krate: String,
    pub version: String,
    pub why: Vec<String>,
    /// Workspace packages that (transitively) pull it in.
    pub roots: Vec<String>,
}

#[derive(Debug, Default)]
pub struct Report {
    pub violations: Vec<Finding>,
    pub tracked: Vec<(Finding, String)>,
    pub accepted: Vec<(Finding, String)>,
    pub exempt: Vec<Finding>,
    pub inactive: Vec<(Finding, String)>,
}

/// `name version` pairs `cargo tree` says are really built.
fn built_set(target: Option<&str>, packages: &[String]) -> Result<BTreeSet<(String, String)>> {
    let mut cmd = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
    cmd.args([
        "tree",
        "--prefix",
        "none",
        "--format",
        "{p}",
        "-e",
        "normal,build",
    ]);
    if packages.is_empty() {
        cmd.arg("--workspace");
    }
    for p in packages {
        cmd.args(["-p", p]);
    }
    if let Some(t) = target {
        cmd.args(["--target", t]);
    }
    let out = cmd.output().context("running `cargo tree`")?;
    if !out.status.success() {
        bail!(
            "cargo tree failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(parse_tree(&String::from_utf8_lossy(&out.stdout)))
}

/// Lines look like `name v1.2.3`, `name v1.2.3 (/path)`, `name v1.2.3 (*)`.
pub fn parse_tree(text: &str) -> BTreeSet<(String, String)> {
    text.lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            let name = it.next()?;
            let ver = it.next()?.strip_prefix('v')?;
            Some((name.to_owned(), ver.to_owned()))
        })
        .collect()
}

pub fn run_strict(
    target: Option<&str>,
    packages: &[String],
    verbose: bool,
    strict: bool,
) -> Result<()> {
    let pol = policy()?;
    let built = built_set(target, packages)?;
    let mut cmd = MetadataCommand::new();
    if let Some(t) = target {
        cmd.other_options(vec!["--filter-platform".into(), t.into()]);
    }
    let meta = cmd.exec()?;
    let report = analyse(&pol, &meta, &built, packages)?;
    print_report(&report, verbose);
    let fail = !report.violations.is_empty() || (strict && !report.tracked.is_empty());
    if fail {
        bail!(
            "{} violation(s){}",
            report.violations.len(),
            if strict {
                format!(" + {} tracked (--strict)", report.tracked.len())
            } else {
                String::new()
            }
        );
    }
    Ok(())
}

pub fn analyse(
    pol: &Policy,
    meta: &cargo_metadata::Metadata,
    built: &BTreeSet<(String, String)>,
    _only: &[String],
) -> Result<Report> {
    let resolve = meta.resolve.as_ref().context("no resolve graph")?;
    let ws: BTreeSet<&PackageId> = meta.workspace_members.iter().collect();
    let pkgs: HashMap<&PackageId, &cargo_metadata::Package> =
        meta.packages.iter().map(|p| (&p.id, p)).collect();
    let nodes: HashMap<&PackageId, &cargo_metadata::Node> =
        resolve.nodes.iter().map(|n| (&n.id, n)).collect();

    // reverse edges (non-dev) restricted to what is really built
    let is_built =
        |id: &PackageId| built.contains(&(pkgs[id].name.to_string(), pkgs[id].version.to_string()));
    let mut rev: HashMap<&PackageId, Vec<&PackageId>> = HashMap::new();
    for n in &resolve.nodes {
        for d in &n.deps {
            if d.dep_kinds
                .iter()
                .all(|k| k.kind == DependencyKind::Development)
            {
                continue;
            }
            rev.entry(&d.pkg).or_default().push(&n.id);
        }
    }
    let roots_of = |start: &PackageId| -> BTreeSet<String> {
        let (mut seen, mut out, mut stack) = (BTreeSet::new(), BTreeSet::new(), vec![start]);
        while let Some(x) = stack.pop() {
            if !seen.insert(x.clone()) {
                continue;
            }
            if ws.contains(x) {
                out.insert(pkgs[x].name.to_string());
                continue;
            }
            if let Some(parents) = rev.get(x) {
                stack.extend(
                    parents
                        .iter()
                        .copied()
                        .filter(|p| ws.contains(*p) || is_built(p)),
                );
            }
        }
        out
    };

    let native: BTreeSet<&str> = pol.native_build_deps.iter().map(String::as_str).collect();
    let mut report = Report::default();
    for (id, node) in &nodes {
        if ws.contains(id) || !is_built(id) {
            continue;
        }
        let p = pkgs[id];
        let mut why = Vec::new();
        // native build helpers actually among this node's resolved build deps
        let active: BTreeSet<String> = node
            .deps
            .iter()
            .filter(|d| d.dep_kinds.iter().any(|k| k.kind == DependencyKind::Build))
            .map(|d| pkgs[&d.pkg].name.to_string())
            .filter(|n| native.contains(n.as_str()))
            .collect();
        if !active.is_empty() {
            why.push(format!(
                "build: {}",
                active.iter().cloned().collect::<Vec<_>>().join(",")
            ));
        }
        if let Some(l) = &p.links {
            if !pol.pure_links.iter().any(|(n, _)| n == p.name.as_str()) {
                why.push(format!("links={l}"));
            }
        }
        if why.is_empty() {
            continue;
        }
        let roots: Vec<String> = roots_of(id).into_iter().collect();
        let f = Finding {
            krate: p.name.to_string(),
            version: p.version.to_string(),
            why,
            roots,
        };
        if let Some((_, feat, reason)) = pol
            .inactive_when
            .iter()
            .find(|(n, feat, _)| n == &f.krate && node.features.iter().any(|x| x == feat))
        {
            let _ = feat;
            report.inactive.push((f, reason.clone()));
        } else if !f.roots.is_empty() && f.roots.iter().all(|r| pol.exempt_roots.contains(r)) {
            report.exempt.push(f);
        } else if let Some((_, why)) = pol.accepted.iter().find(|(n, _)| n == &f.krate) {
            report.accepted.push((f, why.clone()));
        } else if let Some((_, why)) = pol.tracked.iter().find(|(n, _)| n == &f.krate) {
            report.tracked.push((f, why.clone()));
        } else {
            report.violations.push(f);
        }
    }
    report.violations.sort();
    report.tracked.sort();
    report.accepted.sort();
    report.exempt.sort();
    report.inactive.sort();
    Ok(report)
}

fn short(roots: &[String]) -> String {
    let s = roots.join(", ");
    if s.len() > 70 {
        format!("{}…", &s[..70])
    } else {
        s
    }
}

fn print_report(r: &Report, verbose: bool) {
    let line = |f: &Finding| {
        format!(
            "  {} {}  [{}]  <- {}",
            f.krate,
            f.version,
            f.why.join("; "),
            short(&f.roots)
        )
    };
    if r.violations.is_empty() {
        println!("no unexplained C/C++ in the resolved graph");
    } else {
        println!("VIOLATIONS (C/C++ with no policy entry):");
        r.violations.iter().for_each(|f| println!("{}", line(f)));
    }
    if !r.tracked.is_empty() {
        println!("\nTRACKED (known violations with a plan; `--strict` fails on these):");
        for (f, why) in &r.tracked {
            println!("{}\n      plan: {why}", line(f));
        }
    }
    if !r.accepted.is_empty() {
        println!("\nACCEPTED (unavoidable; build headless binaries for musl):");
        for (f, why) in &r.accepted {
            println!("{}\n      why: {why}", line(f));
        }
    }
    if verbose {
        for (title, v) in [("EXEMPT (Slint only)", &r.exempt)] {
            if !v.is_empty() {
                println!("\n{title}:");
                v.iter().for_each(|f| println!("{}", line(f)));
            }
        }
        if !r.inactive.is_empty() {
            println!("\nINACTIVE (native build step switched off):");
            for (f, why) in &r.inactive {
                println!("{}\n      {why}", line(f));
            }
        }
    }
}

/// Group helper used by tests.
#[allow(dead_code)]
pub fn by_name(fs: &[Finding]) -> BTreeMap<&str, &Finding> {
    fs.iter().map(|f| (f.krate.as_str(), f)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_policy_file_parses_and_every_entry_has_a_reason() {
        let p = policy().unwrap();
        assert!(p.native_build_deps.contains(&"cc".to_owned()));
        for (n, why) in p
            .accepted
            .iter()
            .chain(p.tracked.iter())
            .chain(p.pure_links.iter())
        {
            assert!(
                !n.is_empty() && why.len() > 10,
                "{n}: a policy entry needs a real reason"
            );
        }
        assert!(p.inactive_when.iter().all(|(_, _, why)| why.len() > 10));
        // A crate is never both accepted and tracked.
        for (n, _) in &p.accepted {
            assert!(
                !p.tracked.iter().any(|(t, _)| t == n),
                "{n} is in two buckets"
            );
        }
    }

    #[test]
    fn cargo_tree_lines_parse_in_every_shape() {
        let s = "siar-daemon v0.1.0 (/home/x/apps/daemon)\nring v0.17.14\nring v0.17.14 (*)\nblake3 v1.8.5\n\nnonsense\n";
        let set = parse_tree(s);
        assert!(set.contains(&("ring".into(), "0.17.14".into())));
        assert!(set.contains(&("siar-daemon".into(), "0.1.0".into())));
        assert_eq!(set.len(), 3, "duplicates collapse, junk lines are ignored");
    }
}
