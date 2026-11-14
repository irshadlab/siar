//! `cargo xtask android-native` — cross-compile the Android JNI libraries
//! with `cargo-ndk` (replaces `apps/android/build-native.sh`).

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};

pub const ABIS: [&str; 4] = ["arm64-v8a", "armeabi-v7a", "x86_64", "x86"];
/// Matches `app/build.gradle.kts`'s minSdk.
pub const MIN_API: u32 = 26;
pub const CRATES: [&str; 7] = [
    "siar-android-connectivity",
    "siar-android-messaging",
    "siar-transport-wifi-direct",
    "siar-transport-wifi-aware",
    "siar-transport-ble-android",
    "siar-transport-bluetooth-classic",
    "siar-media-android",
];

/// The exact `cargo ndk …` argument list (pure, so it is unit-tested).
pub fn ndk_args(jni_libs: &Path) -> Vec<String> {
    let mut a: Vec<String> = vec!["ndk".into()];
    for abi in ABIS {
        a.extend(["-t".into(), abi.into()]);
    }
    a.extend([
        "-o".into(),
        jni_libs.display().to_string(),
        "-P".into(),
        MIN_API.to_string(),
        "build".into(),
        "--release".into(),
    ]);
    for c in CRATES {
        a.extend(["-p".into(), c.into()]);
    }
    a
}

pub fn run(dry_run: bool) -> Result<()> {
    let root = PathBuf::from(
        cargo_metadata::MetadataCommand::new()
            .no_deps()
            .exec()?
            .workspace_root,
    );
    let jni = root.join("apps/android/app/src/main/jniLibs");
    let args = ndk_args(&jni);
    if dry_run {
        println!("cargo {}", args.join(" "));
        return Ok(());
    }
    let have = Command::new("cargo").args(["ndk", "--version"]).output();
    if !have.is_ok_and(|o| o.status.success()) {
        bail!("cargo-ndk is not installed: `cargo install cargo-ndk` (and set ANDROID_NDK_HOME)");
    }
    std::fs::create_dir_all(&jni)?;
    let st = Command::new("cargo")
        .args(&args)
        .current_dir(&root)
        .status()
        .context("running cargo ndk")?;
    if !st.success() {
        bail!("cargo ndk failed");
    }
    println!("Native libraries written to: {}", jni.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_command_covers_every_abi_and_crate_at_the_right_api_level() {
        let a = ndk_args(Path::new("/x/jniLibs"));
        assert_eq!(a[0], "ndk");
        for abi in ABIS {
            assert!(a.windows(2).any(|w| w == ["-t", abi]), "{abi}");
        }
        for c in CRATES {
            assert!(a.windows(2).any(|w| w == ["-p", c]), "{c}");
        }
        assert!(a.windows(2).any(|w| w == ["-P", "26"]));
        assert!(a.windows(2).any(|w| w == ["-o", "/x/jniLibs"]));
        assert!(a.contains(&"--release".to_owned()) && a.contains(&"build".to_owned()));
    }
}
