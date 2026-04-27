//! Timelapse generation pipeline. Produces pre-rendered fast-playback
//! MP4s per (trip, tier, channel) via an opt-in user-installed ffmpeg.
//!
//! The browser's `<video>` playback rate stutters above ~4x once you
//! factor in three synchronized 4K streams. This module sidesteps that
//! by pre-rendering the fast version as an actual file — playback is
//! always 1x, so it's smooth.
//!
//! Architecture mirrors the `scans/` module: a single background worker
//! on a blocking thread, `Arc<AtomicBool>` cancel flag, progress events
//! batched at ~4 Hz, silent abandon on app close with cleanup-on-start.
//!
//! Stage 1 scope: fixed 8x tier, front channel only. Stages 2 and 3
//! add multi-channel and variable-speed tiers respectively; the DB
//! schema and worker loop structure already account for them so those
//! stages are additive rather than structural.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

pub mod cleanup;
pub mod commands;
pub mod events;
pub mod ffmpeg;
pub mod speed_curve;
pub mod types;
pub mod worker;

/// Shared cancel flag, checked between trips in the worker loop and
/// inside `ffmpeg::encode_trip_channel` during the child-process wait
/// loop so cancel is felt sub-second even mid-encode.
pub type CancelFlag = Arc<AtomicBool>;
