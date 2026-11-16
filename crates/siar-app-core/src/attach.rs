//! Reading a local file for sending as an attachment.
//!
//! The core runs in the daemon, so `path` is read **by the daemon** as the
//! user it runs as. The wire form already insists on an absolute path; this
//! module adds the checks that need the filesystem: a regular file (not a
//! directory, device or FIFO a read could block on) within the attachment
//! size limit. Messages here are written for the person who will see them.

use std::path::Path;

use siar_domain::{MediaType, MAX_ATTACHMENT_BYTES};

/// The wire `MediaType` for a file name. Only the types the protocol names
/// are recognised; everything else is `Other`, which is always valid.
#[must_use]
pub fn media_type_for_path(path: &Path) -> MediaType {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    match ext.as_deref() {
        Some("png") => MediaType::ImagePng,
        Some("jpg" | "jpeg") => MediaType::ImageJpeg,
        Some("webp") => MediaType::ImageWebp,
        Some("opus" | "ogg") => MediaType::AudioOpus,
        Some("mp4" | "m4v") => MediaType::VideoMp4,
        _ => MediaType::Other,
    }
}

/// Read `path` for sending. `Err` is a sentence for the user.
pub async fn read_for_sending(path: &str) -> Result<(Vec<u8>, MediaType), String> {
    let p = Path::new(path);
    if !p.is_absolute() {
        return Err("The file path must be absolute.".into());
    }
    let meta = tokio::fs::metadata(p)
        .await
        .map_err(|_| "That file couldn’t be found or opened.".to_owned())?;
    if !meta.is_file() {
        return Err("Only regular files can be sent.".into());
    }
    if meta.len() == 0 {
        return Err("That file is empty.".into());
    }
    if meta.len() > MAX_ATTACHMENT_BYTES {
        return Err(format!(
            "That file is too large (the limit is {} MiB).",
            MAX_ATTACHMENT_BYTES / (1024 * 1024)
        ));
    }
    let bytes = tokio::fs::read(p)
        .await
        .map_err(|_| "That file couldn’t be read.".to_owned())?;
    Ok((bytes, media_type_for_path(p)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("siar-attach-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn extensions_map_to_media_types_case_insensitively() {
        let t = |n: &str| media_type_for_path(Path::new(n));
        assert_eq!(t("/a/photo.PNG"), MediaType::ImagePng);
        assert_eq!(t("/a/photo.jpeg"), MediaType::ImageJpeg);
        assert_eq!(t("/a/x.webp"), MediaType::ImageWebp);
        assert_eq!(t("/a/v.mp4"), MediaType::VideoMp4);
        assert_eq!(t("/a/notes.txt"), MediaType::Other);
        assert_eq!(t("/a/no_extension"), MediaType::Other);
    }

    #[tokio::test]
    async fn a_regular_file_is_read_with_its_type() {
        let d = scratch("ok");
        let f = d.join("pic.png");
        std::fs::write(&f, b"bytes").unwrap();
        let (bytes, t) = read_for_sending(f.to_str().unwrap()).await.unwrap();
        assert_eq!((bytes.as_slice(), t), (&b"bytes"[..], MediaType::ImagePng));
        std::fs::remove_dir_all(d).ok();
    }

    #[tokio::test]
    async fn bad_targets_are_refused_with_a_sentence() {
        let d = scratch("bad");
        let empty = d.join("empty.bin");
        std::fs::write(&empty, b"").unwrap();
        for (path, want) in [
            ("relative.png".to_owned(), "absolute"),
            (
                d.join("missing.png").to_string_lossy().into_owned(),
                "found",
            ),
            (d.to_string_lossy().into_owned(), "regular"),
            (empty.to_string_lossy().into_owned(), "empty"),
        ] {
            let e = read_for_sending(&path).await.unwrap_err();
            assert!(e.contains(want), "{path}: {e}");
        }
        std::fs::remove_dir_all(d).ok();
    }
}
