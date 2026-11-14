//! `cargo xtask check-musl | build-musl | check-static`.
//!
//! Policy: the workspace is pure Rust except Slint (and Kotlin on Android).
//! The C that cannot be bypassed in headless binaries is `ring` (TLS under
//! iroh). Wherever C must be linked, the binary is built for **musl** and
//! must come out **statically linked**, so there is no glibc ABI coupling and
//! no runtime dependency on the host's C library. GUI (Slint) and desktop
//! audio (ALSA/Opus) stay on the host's glibc: ALSA and GPU stacks are
//! `dlopen`ed at runtime, which a static musl binary cannot do.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};

use crate::elf::{linkage, Linkage};

pub const DEFAULT_TARGET: &str = "x86_64-unknown-linux-musl";
/// Headless binaries built for musl.
pub const HEADLESS: [&str; 4] = [
    "siar-daemon",
    "siar-cli",
    "siar-emergency-node",
    "siar-root",
];

fn cargo() -> String {
    std::env::var("CARGO").unwrap_or_else(|_| "cargo".into())
}

pub fn find_in_path(name: &str, path_var: &str) -> Option<PathBuf> {
    std::env::split_paths(path_var)
        .map(|d| d.join(name))
        .find(|p| p.is_file())
}

/// The musl C compiler the `cc` crate will need for `ring`'s C/asm.
pub fn musl_cc() -> Option<PathBuf> {
    find_in_path("musl-gcc", &std::env::var("PATH").unwrap_or_default())
}

fn sysroot() -> Result<PathBuf> {
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    let out = Command::new(rustc)
        .args(["--print", "sysroot"])
        .output()
        .context("running rustc")?;
    Ok(PathBuf::from(String::from_utf8_lossy(&out.stdout).trim()))
}

/// What is missing for `target`, as human-readable fixes (empty = ready).
pub fn missing(target: &str, sysroot: &Path, musl_gcc: Option<&Path>) -> Vec<String> {
    let mut m = Vec::new();
    if !sysroot.join("lib/rustlib").join(target).is_dir() {
        m.push(format!("the Rust standard library for {target}: `rustup target add {target}` (rust-toolchain.toml lists it, so rustup does this automatically on a fresh checkout)"));
    }
    if musl_gcc.is_none() {
        m.push("a musl C compiler for ring's C/asm: install `musl` (Arch: `pacman -S musl`; Debian/Ubuntu: `musl-tools`) so `musl-gcc` is on PATH".into());
    }
    m
}

pub fn check(target: &str) -> Result<()> {
    let miss = missing(target, &sysroot()?, musl_cc().as_deref());
    if miss.is_empty() {
        println!("ready to build for {target}");
        Ok(())
    } else {
        for m in &miss {
            println!("missing: {m}");
        }
        bail!("{} prerequisite(s) missing for {target}", miss.len())
    }
}

pub fn build(target: &str, release: bool, packages: &[String]) -> Result<()> {
    check(target)?;
    let pkgs: Vec<String> = if packages.is_empty() {
        HEADLESS.iter().map(|s| (*s).to_owned()).collect()
    } else {
        packages.to_vec()
    };
    let mut cmd = Command::new(cargo());
    cmd.args(["build", "--target", target]);
    if release {
        cmd.arg("--release");
    }
    for p in &pkgs {
        cmd.args(["-p", p]);
    }
    // The `cc` crate reads CC_<target with _>; point it at musl-gcc unless set.
    let var = format!("CC_{}", target.replace('-', "_"));
    if std::env::var_os(&var).is_none() {
        if let Some(cc) = musl_cc() {
            cmd.env(&var, cc);
        }
    }
    if !cmd.status().context("running cargo build")?.success() {
        bail!("cargo build --target {target} failed");
    }
    let meta = cargo_metadata::MetadataCommand::new().no_deps().exec()?;
    let dir = PathBuf::from(meta.target_directory)
        .join(target)
        .join(if release { "release" } else { "debug" });
    let mut bad = Vec::new();
    for p in &pkgs {
        // Binary names equal package names for the headless set.
        let exe = dir.join(p);
        if !exe.is_file() {
            continue; // a library-only package given by the user
        }
        let l = linkage(&std::fs::read(&exe)?)?;
        let size = std::fs::metadata(&exe)?.len();
        println!(
            "{:<22} {:>9} bytes  {}",
            p,
            size,
            match &l {
                Linkage::Static => "statically linked".to_owned(),
                Linkage::Dynamic(i) => format!("DYNAMIC (interpreter {i})"),
            }
        );
        if matches!(l, Linkage::Dynamic(_)) {
            bad.push(p.clone());
        }
    }
    if !bad.is_empty() {
        bail!("not statically linked: {}", bad.join(", "));
    }
    Ok(())
}

pub fn check_static(file: &Path) -> Result<()> {
    match linkage(&std::fs::read(file).with_context(|| format!("reading {}", file.display()))?)? {
        Linkage::Static => {
            println!("{}: statically linked", file.display());
            Ok(())
        }
        Linkage::Dynamic(i) => bail!("{}: dynamically linked (interpreter {i})", file.display()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_lookup_finds_only_real_files_in_listed_directories() {
        let d = std::env::temp_dir().join(format!("xt-path-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("musl-gcc"), "").unwrap();
        let p = std::env::join_paths([Path::new("/nonexistent"), d.as_path()]).unwrap();
        assert_eq!(
            find_in_path("musl-gcc", p.to_str().unwrap()),
            Some(d.join("musl-gcc"))
        );
        assert_eq!(find_in_path("nope", p.to_str().unwrap()), None);
        assert_eq!(find_in_path("musl-gcc", ""), None);
    }

    #[test]
    fn missing_reports_each_absent_prerequisite_with_a_fix() {
        let none = missing(DEFAULT_TARGET, Path::new("/nonexistent"), None);
        assert_eq!(none.len(), 2);
        assert!(none[0].contains("rustup target add x86_64-unknown-linux-musl"));
        assert!(none[1].contains("musl-gcc"));
        // With both present, nothing is reported.
        let d = std::env::temp_dir().join(format!("xt-sysroot-{}", std::process::id()));
        std::fs::create_dir_all(d.join("lib/rustlib").join(DEFAULT_TARGET)).unwrap();
        assert!(missing(DEFAULT_TARGET, &d, Some(Path::new("/usr/bin/musl-gcc"))).is_empty());
    }

    #[test]
    fn the_headless_set_excludes_the_gui_and_audio_crates() {
        for gui in ["siar-desktop", "siar-ui-slint", "siar-media-audio"] {
            assert!(!HEADLESS.contains(&gui));
        }
        assert!(HEADLESS.contains(&"siar-daemon"));
    }
}
