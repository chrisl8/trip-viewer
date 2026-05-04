//! Archive-relative path helpers.
//!
//! The per-archive DB stores file locations relative to the archive root
//! and always with forward-slash separators, so a Linux-written archive
//! opens cleanly on Windows and vice versa. Three helpers do the work:
//!
//! - [`to_archive_relative`] strips the archive root prefix from an
//!   absolute `Path` and normalizes separators. Used at every DB write
//!   boundary for fresh data (the scanner just walked the filesystem,
//!   so the path exists on the current machine).
//! - [`relativize_str`] is the string-only sibling used by the legacy
//!   migration: the legacy DB might hold Windows paths like
//!   `Z:\Wolfbox Dashcam\Videos\...` while the migration runs on
//!   Linux, so the path doesn't exist on disk and `Path::strip_prefix`
//!   wouldn't match `Z:\...` against `/run/media/...` anyway.
//! - [`from_archive_relative`] rejoins the archive root with the
//!   stored string. Used at every read boundary.
//!
//! Cross-OS round-trip relies on `Path::join` accepting forward slashes
//! on every platform Rust supports — verified by tests gated on
//! `cfg(windows)` and `cfg(unix)`.

use std::path::{Component, Path, PathBuf};

use crate::error::AppError;

/// Convert an absolute path to its archive-relative storage form.
///
/// Both `abs` and `archive_root` must already be in their canonical
/// forms — `db::open` canonicalizes the archive root once at startup,
/// and the scanner produces absolute paths from `walkdir` that share
/// that canonical shape. This function never touches the filesystem.
///
/// Errors:
/// - [`AppError::PathOutsideArchive`] if `abs` is not a descendant of
///   `archive_root`.
/// - [`AppError::Internal`] for `..` segments after the prefix strip,
///   for an empty result (would alias the root itself), or for any
///   other unexpected component.
pub fn to_archive_relative(abs: &Path, archive_root: &Path) -> Result<String, AppError> {
    let stripped = abs
        .strip_prefix(archive_root)
        .map_err(|_| AppError::PathOutsideArchive {
            path: abs.display().to_string(),
        })?;

    let mut parts: Vec<String> = Vec::new();
    for c in stripped.components() {
        match c {
            Component::Normal(s) => parts.push(s.to_string_lossy().into_owned()),
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(AppError::Internal(format!(
                    "refusing to encode '..' in archive-relative path: {}",
                    abs.display()
                )));
            }
            Component::Prefix(_) | Component::RootDir => {
                return Err(AppError::Internal(format!(
                    "unexpected absolute component after strip_prefix: {}",
                    abs.display()
                )));
            }
        }
    }

    if parts.is_empty() {
        return Err(AppError::Internal(format!(
            "archive-relative path is empty (would alias the archive root): {}",
            abs.display()
        )));
    }

    Ok(parts.join("/"))
}

/// String-only relativization for legacy migration. Strips a known
/// prefix (with case-insensitive matching of the Windows drive letter
/// when both inputs look like Windows paths) and normalizes
/// backslashes to forward slashes. Returns `None` if `abs_str` does
/// not start with `prefix_str`.
///
/// Distinct from [`to_archive_relative`] in three ways:
/// 1. Operates on `&str` so it works on Windows-shaped paths (`Z:\…`)
///    even when the migration is running on Linux.
/// 2. Doesn't require either input to exist on disk.
/// 3. Trailing-slash agnostic for the prefix and case-insensitive on
///    Windows drive letters (`Z:\foo` matches both `Z:\foo` and
///    `z:\foo`) so the user-confirmed archive root and the
///    discovered-from-segments root can match even with cosmetic
///    differences.
pub fn relativize_str(abs_str: &str, prefix_str: &str) -> Option<String> {
    let abs_norm = abs_str.replace('\\', "/");
    let prefix_norm = prefix_str.replace('\\', "/");
    let prefix_norm = prefix_norm.trim_end_matches('/');

    let trimmed = if has_windows_drive_prefix(&abs_norm)
        && has_windows_drive_prefix(prefix_norm)
        && abs_norm.len() >= prefix_norm.len()
        && abs_norm
            .as_bytes()
            .iter()
            .zip(prefix_norm.as_bytes())
            .take(prefix_norm.len())
            .all(|(a, b)| a.eq_ignore_ascii_case(b))
    {
        &abs_norm[prefix_norm.len()..]
    } else {
        abs_norm.strip_prefix(prefix_norm)?
    };

    let stripped = trimmed.trim_start_matches('/');
    if stripped.is_empty() {
        None
    } else {
        Some(stripped.to_string())
    }
}

fn has_windows_drive_prefix(s: &str) -> bool {
    let bytes = s.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

/// Recombine an archive-relative path with its archive root.
///
/// Forward-slash separators in the stored string are interpreted by
/// `Path::join` correctly on every platform — `"a/b/c"` is treated as
/// three components on both Unix and Windows.
pub fn from_archive_relative(rel: &str, archive_root: &Path) -> PathBuf {
    archive_root.join(rel)
}

/// True when `child` lies under `parent` after canonicalization. Used
/// for the "is this scan path part of the active archive?" guard.
/// Both arguments must already exist on disk — returns `false` if
/// either fails to canonicalize.
#[allow(dead_code)] // wired up by the multi-archive switcher.
pub fn is_under(child: &Path, parent: &Path) -> bool {
    let Ok(c) = dunce::canonicalize(child) else {
        return false;
    };
    let Ok(p) = dunce::canonicalize(parent) else {
        return false;
    };
    c.starts_with(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_simple() {
        let root = Path::new("/library");
        let video = Path::new("/library/Videos/2026_01_01.mp4");

        let rel = to_archive_relative(video, root).unwrap();
        assert_eq!(rel, "Videos/2026_01_01.mp4");

        let back = from_archive_relative(&rel, root);
        assert_eq!(back, video);
    }

    #[test]
    fn roundtrip_nested() {
        let root = Path::new("/library");
        let nested = Path::new("/library/Videos/2026/01/clip.mp4");

        let rel = to_archive_relative(nested, root).unwrap();
        assert_eq!(rel, "Videos/2026/01/clip.mp4");

        assert_eq!(from_archive_relative(&rel, root), nested);
    }

    #[test]
    fn rejects_path_outside_archive() {
        let root = Path::new("/library");
        let outsider = Path::new("/elsewhere/video.mp4");
        match to_archive_relative(outsider, root) {
            Err(AppError::PathOutsideArchive { .. }) => {}
            other => panic!("expected PathOutsideArchive, got {other:?}"),
        }
    }

    #[test]
    fn rejects_archive_root_itself() {
        let root = Path::new("/library");
        match to_archive_relative(root, root) {
            Err(AppError::Internal(msg)) => {
                assert!(msg.contains("empty"), "msg was: {msg}");
            }
            other => panic!("expected Internal, got {other:?}"),
        }
    }

    #[test]
    fn relativize_str_strips_unix_prefix() {
        let got = relativize_str(
            "/run/media/chris10/Matrix/Wolfbox Dashcam/Videos/2026_01.MP4",
            "/run/media/chris10/Matrix/Wolfbox Dashcam",
        );
        assert_eq!(got, Some("Videos/2026_01.MP4".to_string()));
    }

    #[test]
    fn relativize_str_normalizes_windows_to_forward_slash() {
        let got = relativize_str(
            r"Z:\Wolfbox Dashcam\Videos\2026_01.MP4",
            r"Z:\Wolfbox Dashcam",
        );
        assert_eq!(got, Some("Videos/2026_01.MP4".to_string()));
    }

    #[test]
    fn relativize_str_case_insensitive_drive_letter() {
        let got = relativize_str(
            r"Z:\Wolfbox Dashcam\Videos\X.mp4",
            r"z:\Wolfbox Dashcam",
        );
        assert_eq!(got, Some("Videos/X.mp4".to_string()));
    }

    #[test]
    fn relativize_str_returns_none_when_outside_prefix() {
        let got = relativize_str("/foo/bar/baz.mp4", "/qux");
        assert_eq!(got, None);
    }

    #[test]
    fn relativize_str_handles_trailing_slash_in_prefix() {
        let got = relativize_str("/library/Videos/X.mp4", "/library/");
        assert_eq!(got, Some("Videos/X.mp4".to_string()));
    }

    #[test]
    fn relativize_str_returns_none_for_root_itself() {
        let got = relativize_str("/library", "/library");
        assert_eq!(got, None);
    }

    #[test]
    fn from_archive_relative_handles_forward_slash() {
        let root = Path::new("/library");
        let p = from_archive_relative("Videos/Front/clip.mp4", root);
        assert_eq!(p, Path::new("/library/Videos/Front/clip.mp4"));
    }
}
