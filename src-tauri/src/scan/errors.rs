//! Classify an `AppError` produced by the scan pipeline into a stable
//! `ScanErrorKind` plus a short, user-facing message.
//!
//! The `mp4` crate produces free-form error strings whose wording varies
//! across versions. Because `mp4` is pinned in `Cargo.toml`, substring
//! matching here is pragmatic: brittle to dep bumps but stable within a
//! version. When bumping the `mp4` crate, re-run against
//! `cargo run --example make_bad_fixtures` output and verify the
//! classifier still maps each case to the intended kind.

use crate::error::AppError;
use crate::model::ScanErrorKind;

/// Result of classifying an error: category + short message + optional
/// raw detail for a future row-expand UI.
pub struct Classification {
    pub kind: ScanErrorKind,
    pub message: String,
    pub detail: Option<String>,
}

pub fn classify(err: &AppError) -> Classification {
    match err {
        AppError::InvalidFilename(_) => Classification {
            kind: ScanErrorKind::InvalidFilename,
            message: "Filename doesn't match any known dashcam format.".to_string(),
            detail: None,
        },
        AppError::Io(e) => Classification {
            kind: ScanErrorKind::FileUnreadable,
            message: "Could not read the file.".to_string(),
            detail: Some(e.to_string()),
        },
        AppError::NotVideo(_) => Classification {
            kind: ScanErrorKind::Mp4NoVideoTrack,
            message: "No video track found in this file.".to_string(),
            detail: None,
        },
        AppError::Parse(s) => classify_mp4_parse(s),
        AppError::Internal(s) => Classification {
            kind: ScanErrorKind::Mp4Other,
            message: "MP4 parse failure.".to_string(),
            detail: Some(s.clone()),
        },
        // These shouldn't surface from the scan pipeline, but keep them
        // mapped so an exhaustive match is future-proof.
        AppError::ImportAlreadyRunning | AppError::NoImportRunning => Classification {
            kind: ScanErrorKind::Mp4Other,
            message: "Unexpected error during scan.".to_string(),
            detail: Some(err.to_string()),
        },
    }
}

fn classify_mp4_parse(raw: &str) -> Classification {
    let detail = Some(raw.to_string());
    if raw.contains("moov not found") {
        Classification {
            kind: ScanErrorKind::Mp4MoovMissing,
            message:
                "Missing index (moov). File was not closed properly — media may be recoverable with external tools."
                    .to_string(),
            detail,
        }
    } else if raw.contains("larger size than it") {
        Classification {
            kind: ScanErrorKind::Mp4BoxOverflow,
            message:
                "Structural corruption: a box declares more data than the file contains. Cut mid-write."
                    .to_string(),
            detail,
        }
    } else {
        Classification {
            kind: ScanErrorKind::Mp4Other,
            message: "MP4 parse failure.".to_string(),
            detail,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_filename_maps() {
        let c = classify(&AppError::InvalidFilename("garbage.mp4".into()));
        assert_eq!(c.kind, ScanErrorKind::InvalidFilename);
        assert!(c.detail.is_none());
        assert!(c.message.contains("Filename"));
    }

    #[test]
    fn io_maps_to_unreadable_with_detail() {
        let io = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied");
        let c = classify(&AppError::Io(io));
        assert_eq!(c.kind, ScanErrorKind::FileUnreadable);
        assert!(c.detail.as_deref().unwrap().contains("denied"));
    }

    #[test]
    fn not_video_maps() {
        let c = classify(&AppError::NotVideo("x.mp4".into()));
        assert_eq!(c.kind, ScanErrorKind::Mp4NoVideoTrack);
    }

    #[test]
    fn moov_missing_classified() {
        let c = classify(&AppError::Parse("moov not found".into()));
        assert_eq!(c.kind, ScanErrorKind::Mp4MoovMissing);
        assert!(c.message.contains("moov"));
        assert_eq!(c.detail.as_deref(), Some("moov not found"));
    }

    #[test]
    fn box_overflow_classified() {
        let c = classify(&AppError::Parse(
            "file contains a box with a larger size than it".into(),
        ));
        assert_eq!(c.kind, ScanErrorKind::Mp4BoxOverflow);
        assert!(c.message.contains("Structural"));
    }

    #[test]
    fn unknown_parse_error_falls_through_to_other() {
        let c = classify(&AppError::Parse("something unfamiliar".into()));
        assert_eq!(c.kind, ScanErrorKind::Mp4Other);
        assert_eq!(c.detail.as_deref(), Some("something unfamiliar"));
    }

    #[test]
    fn internal_maps_to_other_with_detail() {
        let c = classify(&AppError::Internal("oops".into()));
        assert_eq!(c.kind, ScanErrorKind::Mp4Other);
        assert_eq!(c.detail.as_deref(), Some("oops"));
    }
}
