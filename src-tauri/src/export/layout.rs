//! Geometry and ffmpeg argument construction for composite clip export.
//!
//! Everything in here is pure (no I/O, no Tauri) so the layout maths and
//! the filter graph can be unit-tested without spawning ffmpeg.
//!
//! The export takes one *main* channel at 1920x1080 and places the other
//! channels (and optionally a GPS mini-map) next to it or on top of it:
//!
//! - [`LayoutKind::Side`]: the main video keeps the left 1920x1080 and the
//!   small videos are stacked in a column on the right. Nothing is covered
//!   (burned-in date/speed/coordinates stay readable).
//! - [`LayoutKind::Pip`]: picture-in-picture. The canvas stays 1920x1080
//!   and the small videos sit along the top or bottom edge.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const MAIN_W: u32 = 1920;
pub const MAIN_H: u32 = 1080;
/// Every input is resampled to this frame rate before compositing, since
/// channels of one camera can differ (70mai T800: front 25 fps, rear and
/// cabin 30 fps).
pub const OUT_FPS: u32 = 30;
const PIP_MARGIN: u32 = 20;
/// Mini-map is rendered once per second and held; GPS only updates at 1 Hz.
pub const MAP_FPS: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LayoutKind {
    Side,
    Pip,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SlotSize {
    Small,
    Medium,
    Large,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PipEdge {
    Top,
    Bottom,
}

/// Output quality, as an x264 CRF. Dashcam footage is detailed and noisy,
/// so the best setting makes very large files; `Balanced` is the default
/// and `Small` is meant for messaging apps and email.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ExportQuality {
    High,
    Balanced,
    Small,
}

/// Video codec of the exported file. H.264 plays everywhere; H.265 makes
/// files roughly 40% smaller at the same look, but is slower to encode and
/// not every player or messaging app handles it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ExportCodec {
    #[default]
    H264,
    H265,
}

impl ExportQuality {
    /// CRF for the chosen codec. x265's scale is about 3 steps higher than
    /// x264's for the same visual quality.
    pub fn crf(self, codec: ExportCodec) -> u32 {
        let base = match self {
            ExportQuality::High => 20,
            ExportQuality::Balanced => 25,
            ExportQuality::Small => 30,
        };
        match codec {
            ExportCodec::H264 => base,
            ExportCodec::H265 => base + 3,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub canvas_w: u32,
    pub canvas_h: u32,
    pub main: Rect,
    /// One rect per small slot, in the order the slots were requested.
    pub slots: Vec<Rect>,
}

fn even_down(v: u32) -> u32 {
    v & !1
}

/// Compute the canvas and slot rectangles for `n_slots` small tiles.
pub fn plan_layout(kind: LayoutKind, size: SlotSize, edge: PipEdge, n_slots: usize) -> Plan {
    let main = Rect {
        x: 0,
        y: 0,
        w: MAIN_W,
        h: MAIN_H,
    };
    if n_slots == 0 {
        return Plan {
            canvas_w: MAIN_W,
            canvas_h: MAIN_H,
            main,
            slots: vec![],
        };
    }
    let n = n_slots as u32;
    match kind {
        LayoutKind::Side => {
            let requested = match size {
                SlotSize::Small => 270,
                SlotSize::Medium => 360,
                SlotSize::Large => 540,
            };
            // The column can never be taller than the main video.
            let h = even_down(requested.min(MAIN_H / n));
            let w = even_down(h * 16 / 9);
            let slots = (0..n)
                .map(|i| Rect {
                    x: MAIN_W,
                    y: i * h,
                    w,
                    h,
                })
                .collect();
            Plan {
                canvas_w: MAIN_W + w,
                canvas_h: MAIN_H,
                main,
                slots,
            }
        }
        LayoutKind::Pip => {
            let requested = match size {
                SlotSize::Small => 384,
                SlotSize::Medium => 512,
                SlotSize::Large => 640,
            };
            let max_w = (MAIN_W - (n + 1) * PIP_MARGIN) / n;
            let w = even_down(requested.min(max_w));
            let h = even_down(w * 9 / 16);
            let y = match edge {
                PipEdge::Top => PIP_MARGIN,
                PipEdge::Bottom => MAIN_H - PIP_MARGIN - h,
            };
            let slots = (0..n)
                .map(|i| Rect {
                    x: PIP_MARGIN + i * (w + PIP_MARGIN),
                    y,
                    w,
                    h,
                })
                .collect();
            Plan {
                canvas_w: MAIN_W,
                canvas_h: MAIN_H,
                main,
                slots,
            }
        }
    }
}

/// Everything `build_args` needs. Channel inputs are numbered by their
/// position in `channel_lists`; the optional map raw input comes right
/// after them.
pub struct ExportSpec<'a> {
    pub layout: LayoutKind,
    pub slot_size: SlotSize,
    pub pip_edge: PipEdge,
    pub quality: ExportQuality,
    pub codec: ExportCodec,
    /// Where in the concatenated input the clip starts, and how long it is.
    pub trim_start_s: f64,
    pub duration_s: f64,
    /// ffmpeg concat-demuxer list file for each channel.
    pub channel_lists: &'a [PathBuf],
    /// Index into `channel_lists` of the full-size channel.
    pub main: usize,
    /// Indices into `channel_lists` of the small tiles, in display order.
    pub slots: &'a [usize],
    /// Raw rgb24 mini-map frames (1 fps) and their size. The map takes the
    /// last small slot.
    pub map: Option<(&'a Path, u32, u32)>,
    /// (channel index, volume multiplier). Empty = no audio track.
    pub audio: &'a [(usize, f64)],
    pub output: &'a Path,
}

/// Number of small slots including the map, which is what drives layout.
pub fn slot_count(slots: usize, has_map: bool) -> usize {
    slots + usize::from(has_map)
}

/// Dimensions of the map tile for this layout, so the mini-map can be
/// rendered at exactly the size it will be shown at.
pub fn map_tile_size(
    kind: LayoutKind,
    size: SlotSize,
    edge: PipEdge,
    channel_slots: usize,
) -> (u32, u32) {
    let plan = plan_layout(kind, size, edge, slot_count(channel_slots, true));
    let r = plan.slots[plan.slots.len() - 1];
    (r.w, r.h)
}

fn fit(w: u32, h: u32) -> String {
    format!(
        "scale={w}:{h}:force_original_aspect_ratio=decrease,\
         pad={w}:{h}:(ow-iw)/2:(oh-ih)/2,setsar=1"
    )
}

/// Build the `-filter_complex` graph. Returns the graph and whether it
/// produces an `[aout]` audio label.
pub fn build_filter_graph(spec: &ExportSpec<'_>) -> (String, bool) {
    let n_slots = slot_count(spec.slots.len(), spec.map.is_some());
    let plan = plan_layout(spec.layout, spec.slot_size, spec.pip_edge, n_slots);
    let dur = spec.duration_s;
    let mut parts: Vec<String> = Vec::new();

    parts.push(format!(
        "color=c=black:s={}x{}:r={OUT_FPS}:d={dur:.3}[bg]",
        plan.canvas_w, plan.canvas_h
    ));

    // The clip start/length are applied as input options (`-ss`/`-t`, see
    // `build_args`) so ffmpeg seeks instead of decoding everything before
    // the clip; the graph only has to reset timestamps.
    let trimmed = |idx: usize, w: u32, h: u32, label: &str| {
        format!(
            "[{idx}:v]setpts=PTS-STARTPTS,fps={OUT_FPS},{}[{label}]",
            fit(w, h)
        )
    };

    parts.push(trimmed(spec.main, plan.main.w, plan.main.h, "main"));
    for (k, &ch) in spec.slots.iter().enumerate() {
        let r = plan.slots[k];
        parts.push(trimmed(ch, r.w, r.h, &format!("s{k}")));
    }
    if spec.map.is_some() {
        let k = spec.slots.len();
        let r = plan.slots[k];
        let map_idx = spec.channel_lists.len();
        parts.push(format!(
            "[{map_idx}:v]fps={OUT_FPS},{}[s{k}]",
            fit(r.w, r.h)
        ));
    }

    // Chain the overlays: bg <- main <- s0 <- s1 ...
    parts.push("[bg][main]overlay=0:0[o0]".to_string());
    for k in 0..n_slots {
        let r = plan.slots[k];
        parts.push(format!("[o{k}][s{k}]overlay={}:{}[o{}]", r.x, r.y, k + 1));
    }
    parts.push(format!("[o{n_slots}]format=yuv420p[vout]"));

    let has_audio = !spec.audio.is_empty();
    if has_audio {
        for (k, &(ch, vol)) in spec.audio.iter().enumerate() {
            parts.push(format!(
                "[{ch}:a]asetpts=PTS-STARTPTS,volume={vol:.3}[a{k}]"
            ));
        }
        if spec.audio.len() == 1 {
            parts.push("[a0]anull[aout]".to_string());
        } else {
            let n = spec.audio.len();
            let ins: String = (0..n).map(|k| format!("[a{k}]")).collect();
            // amix divides each input by N; multiply back so the chosen
            // volumes mean what the user set (works on every ffmpeg
            // version, unlike amix's `normalize=0`).
            parts.push(format!(
                "{ins}amix=inputs={n}:duration=longest:dropout_transition=0,volume={n}[aout]"
            ));
        }
    }

    (parts.join(";"), has_audio)
}

/// Full ffmpeg argument list (without the binary and without the
/// `-y -hide_banner -progress ...` housekeeping flags, which the runner
/// adds).
pub fn build_args(spec: &ExportSpec<'_>) -> Vec<String> {
    let mut a: Vec<String> = Vec::new();
    for list in spec.channel_lists {
        // Seek to the clip start inside the concatenated channel (the
        // lists carry per-file durations, which is what makes seeking
        // across files possible) and read only as much as the clip needs.
        a.push("-ss".into());
        a.push(format!("{:.3}", spec.trim_start_s));
        a.push("-t".into());
        a.push(format!("{:.3}", spec.duration_s));
        a.extend(["-f", "concat", "-safe", "0", "-i"].map(String::from));
        a.push(list.to_string_lossy().into_owned());
    }
    if let Some((raw, w, h)) = spec.map {
        a.extend(["-f", "rawvideo", "-pixel_format", "rgb24"].map(String::from));
        a.push("-video_size".into());
        a.push(format!("{w}x{h}"));
        a.push("-framerate".into());
        a.push(MAP_FPS.to_string());
        a.push("-i".into());
        a.push(raw.to_string_lossy().into_owned());
    }

    let (graph, has_audio) = build_filter_graph(spec);
    a.push("-filter_complex".into());
    a.push(graph);
    a.extend(["-map", "[vout]"].map(String::from));
    if has_audio {
        a.extend(["-map", "[aout]"].map(String::from));
    } else {
        a.push("-an".into());
    }
    a.push("-t".into());
    a.push(format!("{:.3}", spec.duration_s));
    match spec.codec {
        ExportCodec::H264 => {
            a.extend(["-c:v", "libx264", "-preset", "veryfast", "-crf"].map(String::from));
        }
        ExportCodec::H265 => {
            a.extend(["-c:v", "libx265", "-preset", "veryfast", "-crf"].map(String::from));
        }
    }
    a.push(spec.quality.crf(spec.codec).to_string());
    a.extend(["-pix_fmt", "yuv420p"].map(String::from));
    if spec.codec == ExportCodec::H265 {
        // "hvc1" is what QuickTime, iOS and macOS Finder need to play it.
        a.extend(["-tag:v", "hvc1", "-x265-params", "log-level=error"].map(String::from));
    }
    if has_audio {
        a.extend(["-c:a", "aac", "-b:a", "128k"].map(String::from));
    }
    a.extend(["-movflags", "+faststart"].map(String::from));
    a.push(spec.output.to_string_lossy().into_owned());
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_even(p: &Plan) -> bool {
        let even = |r: &Rect| (r.x | r.y | r.w | r.h) & 1 == 0;
        (p.canvas_w | p.canvas_h) & 1 == 0 && even(&p.main) && p.slots.iter().all(even)
    }

    #[test]
    fn no_slots_is_plain_main() {
        let p = plan_layout(LayoutKind::Side, SlotSize::Medium, PipEdge::Bottom, 0);
        assert_eq!((p.canvas_w, p.canvas_h), (1920, 1080));
        assert!(p.slots.is_empty());
    }

    #[test]
    fn side_medium_three_slots_fills_the_column() {
        let p = plan_layout(LayoutKind::Side, SlotSize::Medium, PipEdge::Bottom, 3);
        assert_eq!((p.canvas_w, p.canvas_h), (2560, 1080));
        assert_eq!(
            p.slots,
            vec![
                Rect {
                    x: 1920,
                    y: 0,
                    w: 640,
                    h: 360
                },
                Rect {
                    x: 1920,
                    y: 360,
                    w: 640,
                    h: 360
                },
                Rect {
                    x: 1920,
                    y: 720,
                    w: 640,
                    h: 360
                },
            ]
        );
        assert!(all_even(&p));
    }

    #[test]
    fn side_large_two_slots_uses_half_height_each() {
        let p = plan_layout(LayoutKind::Side, SlotSize::Large, PipEdge::Bottom, 2);
        assert_eq!((p.canvas_w, p.canvas_h), (1920 + 960, 1080));
        assert_eq!(
            p.slots[1],
            Rect {
                x: 1920,
                y: 540,
                w: 960,
                h: 540
            }
        );
    }

    #[test]
    fn side_column_never_taller_than_main() {
        // 4 slots of 360 would be 1440 tall: clamped to 270.
        let p = plan_layout(LayoutKind::Side, SlotSize::Medium, PipEdge::Bottom, 4);
        assert_eq!(p.slots[0].h, 270);
        assert_eq!(p.slots[3].y + p.slots[3].h, 1080);
        assert_eq!(p.canvas_w, 1920 + 480);
        assert!(all_even(&p));
    }

    #[test]
    fn side_small_leaves_room_below() {
        let p = plan_layout(LayoutKind::Side, SlotSize::Small, PipEdge::Bottom, 2);
        assert_eq!(
            p.slots[0],
            Rect {
                x: 1920,
                y: 0,
                w: 480,
                h: 270
            }
        );
        assert_eq!(p.slots[1].y, 270);
    }

    #[test]
    fn pip_bottom_sits_on_the_bottom_margin() {
        let p = plan_layout(LayoutKind::Pip, SlotSize::Small, PipEdge::Bottom, 2);
        assert_eq!((p.canvas_w, p.canvas_h), (1920, 1080));
        let r = p.slots[0];
        assert_eq!((r.w, r.h), (384, 216));
        assert_eq!(r.y + r.h + 20, 1080);
        assert_eq!(r.x, 20);
        assert_eq!(p.slots[1].x, 20 + 384 + 20);
        assert!(all_even(&p));
    }

    #[test]
    fn pip_top_sits_on_the_top_margin() {
        let p = plan_layout(LayoutKind::Pip, SlotSize::Small, PipEdge::Top, 1);
        assert_eq!(p.slots[0].y, 20);
    }

    #[test]
    fn pip_large_with_many_slots_shrinks_to_fit() {
        let p = plan_layout(LayoutKind::Pip, SlotSize::Large, PipEdge::Bottom, 3);
        let last = p.slots[2];
        assert!(last.x + last.w + 20 <= 1920);
        assert!(all_even(&p));
    }

    fn spec<'a>(
        lists: &'a [PathBuf],
        slots: &'a [usize],
        map: Option<(&'a Path, u32, u32)>,
        audio: &'a [(usize, f64)],
        out: &'a Path,
    ) -> ExportSpec<'a> {
        ExportSpec {
            layout: LayoutKind::Side,
            slot_size: SlotSize::Medium,
            pip_edge: PipEdge::Bottom,
            quality: ExportQuality::Balanced,
            codec: ExportCodec::H264,
            trim_start_s: 12.5,
            duration_s: 30.0,
            channel_lists: lists,
            main: 0,
            slots,
            map,
            audio,
            output: out,
        }
    }

    fn lists3() -> Vec<PathBuf> {
        vec!["/t/f.txt".into(), "/t/c.txt".into(), "/t/r.txt".into()]
    }

    #[test]
    fn graph_for_three_channels_and_map() {
        let lists = lists3();
        let raw = PathBuf::from("/t/map.raw");
        let out = PathBuf::from("/t/out.mp4");
        let s = spec(&lists, &[1, 2], Some((&raw, 640, 360)), &[(0, 1.0)], &out);
        let (g, audio) = build_filter_graph(&s);
        assert!(audio);
        assert!(g.starts_with("color=c=black:s=2560x1080:r=30:d=30.000[bg]"));
        // main + two channel tiles + map tile, in order
        assert!(g.contains("[0:v]setpts=PTS-STARTPTS,fps=30"));
        assert!(g.contains("[1:v]setpts=PTS-STARTPTS,fps=30"));
        assert!(g.contains("[2:v]setpts=PTS-STARTPTS,fps=30"));
        // map is input #3 (right after the three channel lists), no seeking
        assert!(g.contains("[3:v]fps=30,scale=640:360"));
        assert!(g.contains("[o0][s0]overlay=1920:0[o1]"));
        assert!(g.contains("[o1][s1]overlay=1920:360[o2]"));
        assert!(g.contains("[o2][s2]overlay=1920:720[o3]"));
        assert!(g.contains("[o3]format=yuv420p[vout]"));
        assert!(g.contains("[0:a]asetpts=PTS-STARTPTS,volume=1.000[a0]"));
        assert!(!g.contains("trim"));
        assert!(g.ends_with("[a0]anull[aout]"));
    }

    #[test]
    fn graph_without_audio_has_no_audio_chain() {
        let lists = lists3();
        let out = PathBuf::from("/t/out.mp4");
        let s = spec(&lists, &[1], None, &[], &out);
        let (g, audio) = build_filter_graph(&s);
        assert!(!audio);
        assert!(!g.contains("[aout]"));
        assert!(!g.contains(":a]"));
        let args = build_args(&s);
        assert!(args.contains(&"-an".to_string()));
        assert!(!args.contains(&"-c:a".to_string()));
    }

    #[test]
    fn two_audio_sources_are_mixed_with_makeup_gain() {
        let lists = lists3();
        let out = PathBuf::from("/t/out.mp4");
        let s = spec(&lists, &[1], None, &[(0, 1.0), (1, 0.5)], &out);
        let (g, _) = build_filter_graph(&s);
        assert!(g.contains("[1:a]asetpts=PTS-STARTPTS,volume=0.500[a1]"));
        assert!(g.contains("volume=0.500[a1]"));
        assert!(g.contains(
            "[a0][a1]amix=inputs=2:duration=longest:dropout_transition=0,volume=2[aout]"
        ));
    }

    #[test]
    fn args_list_inputs_in_order_and_end_with_output() {
        let lists = lists3();
        let raw = PathBuf::from("/t/map.raw");
        let out = PathBuf::from("/t/out.mp4");
        let s = spec(&lists, &[1, 2], Some((&raw, 640, 360)), &[(0, 1.0)], &out);
        let a = build_args(&s);
        let joined = a.join(" ");
        assert!(joined.starts_with(
            "-ss 12.500 -t 30.000 -f concat -safe 0 -i /t/f.txt \
             -ss 12.500 -t 30.000 -f concat -safe 0 -i /t/c.txt"
        ));
        // the map input is never seeked: its frames already start at clip time 0
        assert!(joined.contains("-t 30.000 -f concat -safe 0 -i /t/r.txt -f rawvideo"));
        assert!(joined.contains(
            "-f rawvideo -pixel_format rgb24 -video_size 640x360 -framerate 1 -i /t/map.raw"
        ));
        assert!(joined.contains("-map [vout] -map [aout]"));
        assert!(joined.contains("-t 30.000"));
        assert_eq!(a.last().unwrap(), "/t/out.mp4");
    }

    #[test]
    fn quality_sets_the_crf() {
        let lists = lists3();
        let out = PathBuf::from("/t/out.mp4");
        let crf_of = |q: ExportQuality| {
            let mut s = spec(&lists, &[1], None, &[], &out);
            s.quality = q;
            let a = build_args(&s);
            let i = a.iter().position(|x| x == "-crf").unwrap();
            a[i + 1].clone()
        };
        assert_eq!(crf_of(ExportQuality::High), "20");
        assert_eq!(crf_of(ExportQuality::Balanced), "25");
        assert_eq!(crf_of(ExportQuality::Small), "30");
    }

    #[test]
    fn h265_uses_x265_with_the_hvc1_tag() {
        let lists = lists3();
        let out = PathBuf::from("/t/out.mp4");
        let mut s = spec(&lists, &[1], None, &[], &out);
        s.codec = ExportCodec::H265;
        let a = build_args(&s);
        let joined = a.join(" ");
        assert!(joined.contains("-c:v libx265"));
        assert!(!joined.contains("libx264"));
        assert!(joined.contains("-tag:v hvc1"));
        let i = a.iter().position(|x| x == "-crf").unwrap();
        assert_eq!(a[i + 1], "28");
        // The default stays H.264 with no tag.
        let d = build_args(&spec(&lists, &[1], None, &[], &out)).join(" ");
        assert!(d.contains("-c:v libx264") && !d.contains("hvc1"));
    }

    #[test]
    fn map_tile_size_matches_plan() {
        // T800: two channel tiles + map in the side column, medium.
        assert_eq!(
            map_tile_size(LayoutKind::Side, SlotSize::Medium, PipEdge::Bottom, 2),
            (640, 360)
        );
        assert_eq!(
            map_tile_size(LayoutKind::Pip, SlotSize::Small, PipEdge::Top, 2),
            (384, 216)
        );
    }
}
