//! Shared types for the timelapse pipeline. Kept separate from the
//! worker and command layers so frontend-facing structs aren't buried
//! inside implementation files.

use serde::{Deserialize, Serialize};

/// Timelapse speed tier. Each tier maps to a base playback rate and,
/// for variable tiers, a slower "event" rate applied during GPS-detected
/// interesting moments. Stage 1 only implements the fixed `Tier8x`.
//
// `Tier8x` / `Tier16x` / `Tier60x` repeat the enum name because variant
// names can't start with a digit. The wire format uses bare "8x"/"16x"/"60x"
// via the serde rename attribute.
#[allow(clippy::enum_variant_names)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Tier {
    #[serde(rename = "8x")]
    Tier8x,
    #[serde(rename = "16x")]
    Tier16x,
    #[serde(rename = "60x")]
    Tier60x,
}

impl Tier {
    pub fn as_str(&self) -> &'static str {
        match self {
            Tier::Tier8x => "8x",
            Tier::Tier16x => "16x",
            Tier::Tier60x => "60x",
        }
    }

    /// The rate applied to "boring" footage in this tier. For fixed
    /// tiers this is the only rate used; for variable tiers it's the
    /// rate outside of GPS-detected event windows.
    pub fn base_rate(&self) -> u32 {
        match self {
            Tier::Tier8x => 8,
            Tier::Tier16x => 16,
            Tier::Tier60x => 60,
        }
    }

    /// The rate applied *inside* event windows for variable tiers.
    /// For fixed tiers this equals `base_rate`.
    ///
    /// - 8x (fixed): 8x in events too (no slowdown).
    /// - 16x (variable): 1x in events — full-speed playback at hard
    ///   brakes / sharp turns so the user can actually see what happened.
    /// - 60x (variable): 8x in events — cinematic pacing for the
    ///   year-in-review where events still feel like moments rather
    ///   than a stop-and-start.
    pub fn event_rate(&self) -> u32 {
        match self {
            Tier::Tier8x => 8,
            Tier::Tier16x => 1,
            Tier::Tier60x => 8,
        }
    }

    /// Whether this tier applies a slower rate during event windows.
    pub fn is_variable(&self) -> bool {
        self.base_rate() != self.event_rate()
    }

    #[allow(dead_code)] // consumed by the frontend->backend tier-string roundtrip
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "8x" => Some(Tier::Tier8x),
            "16x" => Some(Tier::Tier16x),
            "60x" => Some(Tier::Tier60x),
            _ => None,
        }
    }
}

/// Which channel to encode. The filename convention uses a single
/// character (F/I/R) so we match that on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Channel {
    #[serde(rename = "F")]
    Front,
    #[serde(rename = "I")]
    Interior,
    #[serde(rename = "R")]
    Rear,
}

impl Channel {
    pub fn as_str(&self) -> &'static str {
        match self {
            Channel::Front => "F",
            Channel::Interior => "I",
            Channel::Rear => "R",
        }
    }

    /// Canonical segment-label used by the folder scanner
    /// (`model::LABEL_FRONT` etc.). Used by the sibling resolver to
    /// match against `ParsedName.channel_label`.
    pub fn label(&self) -> &'static str {
        match self {
            Channel::Front => crate::model::LABEL_FRONT,
            Channel::Interior => crate::model::LABEL_INTERIOR,
            Channel::Rear => crate::model::LABEL_REAR,
        }
    }
}

/// Which existing jobs should be re-run. Mirrors `scans::ScanScope` but
/// tailored to the timelapse state machine: "new" means no row yet,
/// "failed" only retries the failed ones, "all" rebuilds everything.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum JobScope {
    NewOnly,
    FailedOnly,
    RebuildAll,
}

/// A window of concat-timeline seconds where playback should slow to
/// the tier's `event_rate()` instead of its `base_rate()`. Produced
/// by `events::detect_events` and consumed by `speed_curve::compose_filter`.
///
/// Timeline note: the GPS input is remapped to concat-timeline (sum of
/// prior segment durations) before event detection, so these offsets
/// line up with ffmpeg's `trim` filter on the concat-demuxer output.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EventWindow {
    pub start_s: f64,
    pub end_s: f64,
}

/// Capabilities reported by a configured ffmpeg binary. Populated by
/// `ffmpeg::probe_ffmpeg` when the user runs the Test button in the
/// settings dialog, and cached in the `settings` table so subsequent
/// launches don't re-probe.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FfmpegCapabilities {
    /// Full version string reported by `ffmpeg -version`'s first line.
    /// Displayed in the settings dialog for sanity-check.
    pub version: String,
    /// Whether `hevc_nvenc` appeared in `ffmpeg -encoders` output.
    /// The worker prefers NVENC when available (15-40x realtime) and
    /// falls back to `libx265` (0.5-2x realtime) when it isn't.
    pub nvenc_hevc: bool,
}
