//! Trip-level Tauri commands. These complement the per-segment actions
//! in `tags::commands` (e.g. `delete_segments_to_trash`) and the
//! filesystem-level operations in `scan` and `import` by operating on
//! whole trips: listing the archive-only trips that need surfacing in
//! the UI even though no source segments remain on disk, and the
//! wholesale "delete this entire trip including its timelapse archive"
//! action.
pub mod commands;
pub mod merge;
