//! `cargo xtask gen-mls-storage`
//!
//! `crates/siar-crypto-mls` persists MLS state with a write-through wrapper
//! (`FileStorage`) around `openmls_memory_storage::MemoryStorage`: every
//! successful mutation is followed by an atomic on-disk snapshot, so state is
//! durable before any caller sees the result (an un-persisted ratchet
//! advance would risk key reuse after a crash).
//!
//! The wrapper must implement all of `StorageProvider` (72 methods). Writing
//! that by hand is error-prone and goes stale when openmls changes the
//! trait, so the delegating impl is **generated from the trait's own
//! source**, and `--check` (for CI) fails if the committed file drifts.

use std::fmt::Write as _;
use std::fs;
use std::path::PathBuf;

use anyhow::{anyhow, bail, Context, Result};

const OUT: &str = "crates/siar-crypto-mls/src/persistent_delegate.rs";
const MUTATING: [&str; 6] = [
    "write_", "append_", "queue_", "remove_", "delete_", "clear_",
];

#[derive(Debug, PartialEq, Eq)]
struct Method {
    name: String,
    /// Full generics including `<` `>` (empty if none), `VERSION` rewritten.
    generics: String,
    generic_names: Vec<String>,
    /// Parameters after `&self`, verbatim.
    params: Vec<String>,
    arg_names: Vec<String>,
    ret: String,
    /// `#[cfg(...)]` attribute lines that gate this method in the trait. They
    /// are copied onto the generated method; this crate declares features of
    /// the same names that forward to `openmls_traits` and
    /// `openmls_memory_storage`, so the two stay in step.
    cfg: Vec<String>,
}

/// Index just past the bracket that closes the one at `open`, counting only
/// the given pair. `s[open]` must be `o`.
fn matching(s: &str, open: usize, o: char, c: char) -> Option<usize> {
    let mut depth = 0usize;
    for (i, ch) in s[open..].char_indices() {
        if ch == o {
            depth += 1;
        } else if ch == c {
            depth -= 1;
            if depth == 0 {
                return Some(open + i + c.len_utf8());
            }
        }
    }
    None
}

/// Split at commas that are not inside (), <> or [].
fn split_top_level(s: &str) -> Vec<String> {
    let (mut out, mut cur, mut depth) = (Vec::new(), String::new(), 0i32);
    let mut prev = ' ';
    for ch in s.chars() {
        match ch {
            '(' | '<' | '[' => depth += 1,
            ')' | ']' => depth -= 1,
            '>' if prev != '-' => depth -= 1,
            _ => {}
        }
        if ch == ',' && depth == 0 {
            out.push(cur.trim().to_owned());
            cur.clear();
        } else {
            cur.push(ch);
        }
        prev = ch;
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_owned());
    }
    out
}

fn replace_word(s: &str, word: &str, with: &str) -> String {
    let is_id = |c: char| c.is_alphanumeric() || c == '_';
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find(word) {
        let before = rest[..i].chars().next_back();
        let after = rest[i + word.len()..].chars().next();
        out.push_str(&rest[..i]);
        if before.is_some_and(is_id) || after.is_some_and(is_id) {
            out.push_str(word);
        } else {
            out.push_str(with);
        }
        rest = &rest[i + word.len()..];
    }
    out.push_str(rest);
    out
}

fn parse_methods(src: &str) -> Result<Vec<Method>> {
    let head = "pub trait StorageProvider<const VERSION: u16> {";
    let start = src.find(head).ok_or_else(|| anyhow!("trait not found"))?;
    let body = &src[start..];
    let mut methods = Vec::new();
    let mut from = 0;
    while let Some(i) = body[from..].find("\n    fn ") {
        let at = from + i + "\n    fn ".len();
        // Attributes sit between the previous method's end and this `fn`.
        let cfg: Vec<String> = body[from..from + i]
            .lines()
            .map(str::trim)
            .filter(|l| l.starts_with("#[cfg("))
            .map(str::to_owned)
            .collect();
        from = at;
        let sig = &body[at..];
        let name_end = sig
            .find(['<', '('])
            .ok_or_else(|| anyhow!("malformed signature"))?;
        let name = sig[..name_end].trim().to_owned();
        let (generics, after_generics) = if sig[name_end..].starts_with('<') {
            let end = matching(sig, name_end, '<', '>')
                .ok_or_else(|| anyhow!("unbalanced <> in {name}"))?;
            (sig[name_end..end].to_owned(), end)
        } else {
            (String::new(), name_end)
        };
        let popen = after_generics
            + sig[after_generics..]
                .find('(')
                .ok_or_else(|| anyhow!("no params in {name}"))?;
        let pend =
            matching(sig, popen, '(', ')').ok_or_else(|| anyhow!("unbalanced () in {name}"))?;
        let params_text = &sig[popen + 1..pend - 1];
        // What follows the parameter list: `-> Ret;` (required) or `-> Ret {` (default body).
        let tail_end = sig[pend..]
            .find([';', '{'])
            .ok_or_else(|| anyhow!("no terminator for {name}"))?;
        if sig[pend + tail_end..].starts_with('{') {
            continue; // a provided method (e.g. `version`): not ours to delegate
        }
        let ret = sig[pend..pend + tail_end]
            .trim()
            .strip_prefix("->")
            .ok_or_else(|| anyhow!("{name} has no return type"))?
            .trim()
            .to_owned();

        let generic_names = if generics.is_empty() {
            Vec::new()
        } else {
            split_top_level(&generics[1..generics.len() - 1])
                .iter()
                .map(|g| g.split(':').next().unwrap_or("").trim().to_owned())
                .collect()
        };
        let all_params = split_top_level(params_text);
        let params: Vec<String> = all_params
            .into_iter()
            .filter(|p| p.replace(' ', "") != "&self")
            .collect();
        let arg_names = params
            .iter()
            .map(|p| p.split(':').next().unwrap_or("").trim().to_owned())
            .collect();
        methods.push(Method {
            name,
            generics: replace_word(&generics, "VERSION", "CURRENT_VERSION"),
            generic_names,
            params: params
                .iter()
                .map(|p| replace_word(p, "VERSION", "CURRENT_VERSION"))
                .collect(),
            arg_names,
            ret: replace_word(&ret, "VERSION", "CURRENT_VERSION"),
            cfg,
        });
    }
    if methods.is_empty() {
        bail!("no methods parsed — the trait layout changed; update xtask/src/mls_storage.rs");
    }
    Ok(methods)
}

fn render(methods: &[Method], traits_version: &str) -> String {
    let mut o = String::new();
    let _ = writeln!(
        o,
        "// @generated by `cargo xtask gen-mls-storage` from openmls_traits {traits_version}.\n\
         // DO NOT EDIT. CI runs `cargo xtask gen-mls-storage --check`.\n\
         //\n\
         // Every `StorageProvider` method delegates to the in-memory store; after each\n\
         // successful MUTATING call the state is written to disk (see `persistent.rs`)\n\
         // before the call returns, so nothing is ever acknowledged that is not durable.\n\
         \n\
         #![allow(clippy::too_many_arguments, clippy::type_complexity)]\n\
         \n\
         use openmls_memory_storage::MemoryStorage;\n\
         use openmls_traits::storage::{{traits, StorageProvider, CURRENT_VERSION}};\n\
         \n\
         use super::{{FileStorage, FileStorageError}};\n\
         \n\
         impl StorageProvider<CURRENT_VERSION> for FileStorage {{\n    type Error = FileStorageError;\n"
    );
    for m in methods {
        let mutating = MUTATING.iter().any(|p| m.name.starts_with(p));
        let params = if m.params.is_empty() {
            "&self".to_owned()
        } else {
            format!("&self, {}", m.params.join(", "))
        };
        let turbofish = if m.generic_names.is_empty() {
            String::new()
        } else {
            format!("::<{}>", m.generic_names.join(", "))
        };
        let args = std::iter::once("&self.inner".to_owned())
            .chain(m.arg_names.iter().cloned())
            .collect::<Vec<_>>()
            .join(", ");
        for attr in &m.cfg {
            let _ = writeln!(o, "\n    {attr}");
        }
        let _ = writeln!(
            o,
            "{}    fn {}{}({}) -> {} {{\n        let result = <MemoryStorage as StorageProvider<CURRENT_VERSION>>::{}{}({});",
            if m.cfg.is_empty() { "\n" } else { "" },
            m.name, m.generics, params, m.ret, m.name, turbofish, args
        );
        if mutating {
            let _ = writeln!(
                o,
                "        if result.is_ok() {{\n            self.persist().map_err(FileStorageError::Persist)?;\n        }}"
            );
        }
        let _ = writeln!(
            o,
            "        result.map_err(FileStorageError::Memory)\n    }}"
        );
    }
    let _ = writeln!(o, "}}");
    o
}

fn workspace_root() -> Result<PathBuf> {
    Ok(PathBuf::from(
        cargo_metadata::MetadataCommand::new()
            .no_deps()
            .exec()?
            .workspace_root,
    ))
}

pub fn run(check: bool) -> Result<()> {
    let meta = cargo_metadata::MetadataCommand::new().exec()?;
    let pkg = meta
        .packages
        .iter()
        .find(|p| p.name.as_str() == "openmls_traits")
        .ok_or_else(|| anyhow!("openmls_traits is not in the dependency graph"))?;
    let src_path = pkg
        .manifest_path
        .parent()
        .ok_or_else(|| anyhow!("bad manifest path"))?
        .join("src/storage.rs");
    let src = fs::read_to_string(&src_path).with_context(|| format!("reading {src_path}"))?;
    let methods = parse_methods(&src)?;
    let text = render(&methods, &pkg.version.to_string());
    let out = workspace_root()?.join(OUT);
    if check {
        let have = fs::read_to_string(&out).unwrap_or_default();
        if have != text {
            bail!(
                "{OUT} is out of date with openmls_traits {} — run `cargo xtask gen-mls-storage`",
                pkg.version
            );
        }
        println!("ok: {OUT} is current ({} methods)", methods.len());
    } else {
        fs::write(&out, &text)?;
        println!(
            "wrote {OUT}: {} methods from openmls_traits {}",
            methods.len(),
            pkg.version
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
pub trait StorageProvider<const VERSION: u16> {
    type Error: core::fmt::Debug + std::error::Error;

    fn version() -> u16 {
        VERSION
    }

    /// doc
    fn write_tree<GroupId: traits::GroupId<VERSION>, TreeSync: traits::TreeSync<VERSION>>(
        &self,
        group_id: &GroupId,
        tree: &TreeSync,
    ) -> Result<(), Self::Error>;

    fn group_state<
        GroupState: traits::GroupState<VERSION>,
        GroupId: traits::GroupId<VERSION>,
    >(
        &self,
        group_id: &GroupId,
    ) -> Result<Option<GroupState>, Self::Error>;

    #[cfg(feature = "extensions-draft")]
    fn write_gated<GroupId: traits::GroupId<VERSION>>(
        &self,
        group_id: &GroupId,
    ) -> Result<(), Self::Error>;

    fn clear_proposal_queue<GroupId: traits::GroupId<VERSION>, ProposalRef: traits::ProposalRef<VERSION>>(
        &self,
        group_id: &GroupId,
    ) -> Result<(), Self::Error>;
}
"#;

    #[test]
    fn parses_signatures_and_skips_provided_methods() {
        let m = parse_methods(SAMPLE).unwrap();
        assert_eq!(
            m.iter().map(|x| x.name.as_str()).collect::<Vec<_>>(),
            [
                "write_tree",
                "group_state",
                "write_gated",
                "clear_proposal_queue"
            ]
        );
        assert!(m[0].cfg.is_empty() && m[1].cfg.is_empty() && m[3].cfg.is_empty());
        assert_eq!(
            m[2].cfg,
            [r#"#[cfg(feature = "extensions-draft")]"#],
            "a gate must be remembered"
        );
        assert_eq!(m[0].generic_names, ["GroupId", "TreeSync"]);
        assert_eq!(m[0].arg_names, ["group_id", "tree"]);
        assert_eq!(
            m[1].generic_names,
            ["GroupState", "GroupId"],
            "order matters for the turbofish"
        );
        assert_eq!(m[1].ret, "Result<Option<GroupState>, Self::Error>");
        assert!(m[0].generics.contains("CURRENT_VERSION") && !m[0].generics.contains("<VERSION>"));
    }

    #[test]
    fn mutating_calls_persist_and_reads_do_not() {
        let text = render(&parse_methods(SAMPLE).unwrap(), "0.0.0");
        let write = text
            .split("fn write_tree")
            .nth(1)
            .unwrap()
            .split("fn group_state")
            .next()
            .unwrap();
        assert!(
            write.contains("self.persist()"),
            "writes must persist before returning"
        );
        let read = text
            .split("fn group_state")
            .nth(1)
            .unwrap()
            .split("fn write_gated")
            .next()
            .unwrap();
        assert!(
            !read.contains("self.persist()"),
            "reads must not touch the disk"
        );
        let clear = text.split("fn clear_proposal_queue").nth(1).unwrap();
        assert!(clear.contains("self.persist()"), "clear_ is a mutation too");
        assert!(text.contains("::group_state::<GroupState, GroupId>(&self.inner, group_id)"));
    }

    #[test]
    fn gated_methods_keep_their_gate_in_the_output() {
        let text = render(&parse_methods(SAMPLE).unwrap(), "0.0.0");
        let at = text.find("fn write_gated").unwrap();
        assert!(
            text[..at]
                .trim_end()
                .ends_with(r#"#[cfg(feature = "extensions-draft")]"#),
            "the gate must sit directly above the gated method"
        );
        // …and does not leak onto the next, ungated method.
        let next = text.find("fn clear_proposal_queue").unwrap();
        assert!(!text[..next]
            .trim_end()
            .ends_with(r#"#[cfg(feature = "extensions-draft")]"#));
    }

    #[test]
    fn a_changed_trait_layout_fails_loudly_instead_of_generating_nothing() {
        assert!(parse_methods("pub trait Other {}").is_err());
        assert!(parse_methods(
            "pub trait StorageProvider<const VERSION: u16> {\n    type Error;\n}"
        )
        .is_err());
    }

    #[test]
    fn word_replacement_does_not_touch_longer_identifiers() {
        assert_eq!(
            replace_word("A<VERSION>, CURRENT_VERSION, VERSIONED", "VERSION", "X"),
            "A<X>, CURRENT_VERSION, VERSIONED"
        );
    }
}
