//! Composite clip export: renders a time range of a trip as one MP4 with
//! the channels of the camera combined (main video plus the others small,
//! optionally a GPS mini-map), using the user-supplied ffmpeg.
//!
//! - `layout`: canvas geometry and the ffmpeg filter graph / arguments.
//! - `minimap`: route-and-dot map rendered from GPS points.
//! - `tiles`: optional OpenStreetMap backdrop for the map (a few tiles,
//!   fetched once per export, attribution burned in).
//! - `plan`: turns a request into the concrete segments/channels/audio.
//! - `runner`: spawns ffmpeg with progress and cancel.
//! - `pipeline`: ties them together with a scratch directory.
//! - `commands`: the Tauri commands and events.

pub mod commands;
pub mod layout;
pub mod minimap;
pub mod pipeline;
pub mod plan;
pub mod runner;
pub mod tiles;
