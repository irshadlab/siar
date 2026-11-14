//! Minimal ELF reader: is this executable statically linked?
//!
//! A dynamically linked executable names an interpreter (`PT_INTERP`, e.g.
//! `/lib64/ld-linux-x86-64.so.2`); a static one (including static-PIE) does
//! not. That is exactly the property the musl build must have, and checking
//! it needs no `ldd`/`file` — just the program headers.

use anyhow::{bail, Result};

#[derive(Debug, PartialEq, Eq)]
pub enum Linkage {
    Static,
    /// Dynamic, with its interpreter path.
    Dynamic(String),
}

const PT_INTERP: u32 = 3;

fn u16le(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(
        b.get(at..at.checked_add(2)?)?.try_into().ok()?,
    ))
}
fn u32le(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        b.get(at..at.checked_add(4)?)?.try_into().ok()?,
    ))
}
fn u64le(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(
        b.get(at..at.checked_add(8)?)?.try_into().ok()?,
    ))
}

/// ELF64, little-endian only (x86_64 and aarch64 Linux). Anything else, or
/// anything truncated or inconsistent, is an error — never a panic.
pub fn linkage(bytes: &[u8]) -> Result<Linkage> {
    if bytes.get(..4) != Some(b"\x7fELF") {
        bail!("not an ELF file");
    }
    if bytes.get(4) != Some(&2) || bytes.get(5) != Some(&1) {
        bail!("only little-endian ELF64 is supported");
    }
    let (Some(phoff), Some(phentsize), Some(phnum)) =
        (u64le(bytes, 0x20), u16le(bytes, 0x36), u16le(bytes, 0x38))
    else {
        bail!("truncated ELF header");
    };
    let (phoff, phentsize, phnum) = (
        usize::try_from(phoff)?,
        usize::from(phentsize),
        usize::from(phnum),
    );
    if phentsize < 56 {
        bail!("implausible program header size {phentsize}");
    }
    for i in 0..phnum {
        let at = phoff
            .checked_add(
                i.checked_mul(phentsize)
                    .ok_or_else(|| anyhow::anyhow!("overflow"))?,
            )
            .ok_or_else(|| anyhow::anyhow!("overflow"))?;
        let Some(ptype) = u32le(bytes, at) else {
            bail!("program header {i} is past the end of the file");
        };
        if ptype == PT_INTERP {
            let (Some(off), Some(size)) = (u64le(bytes, at + 8), u64le(bytes, at + 32)) else {
                bail!("truncated PT_INTERP header");
            };
            let (off, size) = (usize::try_from(off)?, usize::try_from(size)?);
            let raw = off
                .checked_add(size)
                .and_then(|end| bytes.get(off..end))
                .ok_or_else(|| anyhow::anyhow!("PT_INTERP points outside the file"))?;
            let path = String::from_utf8_lossy(raw)
                .trim_end_matches('\0')
                .to_owned();
            return Ok(Linkage::Dynamic(path));
        }
    }
    Ok(Linkage::Static)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny synthetic ELF64 with the given program-header types.
    fn elf(types: &[u32], interp: Option<&str>) -> Vec<u8> {
        let phoff = 64usize;
        let mut b = vec![0u8; phoff + types.len() * 56];
        b[..4].copy_from_slice(b"\x7fELF");
        b[4] = 2;
        b[5] = 1;
        b[0x20..0x28].copy_from_slice(&(phoff as u64).to_le_bytes());
        b[0x36..0x38].copy_from_slice(&56u16.to_le_bytes());
        b[0x38..0x3a].copy_from_slice(&(types.len() as u16).to_le_bytes());
        let tail = b.len();
        for (i, t) in types.iter().enumerate() {
            let at = phoff + i * 56;
            b[at..at + 4].copy_from_slice(&t.to_le_bytes());
            if *t == PT_INTERP {
                let s = interp.unwrap_or("/lib/ld.so");
                b[at + 8..at + 16].copy_from_slice(&(tail as u64).to_le_bytes());
                b[at + 32..at + 40].copy_from_slice(&((s.len() + 1) as u64).to_le_bytes());
            }
        }
        if let Some(s) = interp {
            if types.contains(&PT_INTERP) {
                b.extend_from_slice(s.as_bytes());
                b.push(0);
            }
        }
        b
    }

    #[test]
    fn a_binary_without_an_interpreter_is_static_and_one_with_is_dynamic() {
        assert_eq!(
            linkage(&elf(&[1, 2], None)).unwrap(),
            Linkage::Static,
            "static-PIE has PT_DYNAMIC but no interp"
        );
        assert_eq!(linkage(&elf(&[1], None)).unwrap(), Linkage::Static);
        assert_eq!(
            linkage(&elf(&[6, 3, 1], Some("/lib64/ld-linux-x86-64.so.2"))).unwrap(),
            Linkage::Dynamic("/lib64/ld-linux-x86-64.so.2".into())
        );
    }

    #[test]
    fn malformed_input_is_an_error_never_a_panic() {
        for bad in [&b""[..], b"hello", b"\x7fELF", b"\x7fELF\x01\x01"] {
            assert!(linkage(bad).is_err());
        }
        let good = elf(&[3], Some("/lib/ld.so"));
        // every truncation of a valid file
        for n in 0..good.len() {
            let _ = linkage(&good[..n]);
        }
        // hostile header fields
        // A lying header count is only reachable when no interpreter is found
        // first, so use a static-looking file for it.
        let mut h = elf(&[1], None);
        h[0x38..0x3a].copy_from_slice(&u16::MAX.to_le_bytes());
        assert!(linkage(&h).is_err(), "phnum beyond the file");
        let mut h = good.clone();
        h[0x20..0x28].copy_from_slice(&u64::MAX.to_le_bytes());
        assert!(linkage(&h).is_err(), "phoff overflow");
        let mut h = good;
        h[0x36..0x38].copy_from_slice(&8u16.to_le_bytes());
        assert!(linkage(&h).is_err(), "implausible phentsize");
        // big-endian / 32-bit are refused rather than misread
        let mut be = elf(&[1], None);
        be[5] = 2;
        assert!(linkage(&be).is_err());
    }

    #[test]
    fn this_very_test_binary_is_dynamic_on_a_glibc_host() {
        // The test executable is glibc-linked in this environment; skip
        // gracefully elsewhere (musl hosts, macOS).
        if let Ok(bytes) = std::fs::read(std::env::current_exe().unwrap()) {
            if let Ok(l) = linkage(&bytes) {
                if cfg!(all(target_os = "linux", target_env = "gnu")) {
                    assert!(matches!(l, Linkage::Dynamic(_)), "{l:?}");
                }
            }
        }
    }
}
