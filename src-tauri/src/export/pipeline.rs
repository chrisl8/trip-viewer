//! The export itself, minus the Tauri glue: prepare the plan, write the
//! concat lists, render the mini-map, run ffmpeg, move the result into
//! place. Everything lives in a scratch directory that is removed on the
//! way out, success or not.

use std::fs;
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::error::AppError;
use crate::export::layout::{build_args, map_tile_size, ExportSpec};
use crate::export::minimap::{render_frames, Backdrop, TrackPoint};
use crate::export::plan::{prepare, MapStyle, Prepared, StartExportArgs};
use crate::export::runner::run_with_progress;
use crate::export::tiles::{choose_view, render_backdrop, HttpTiles, TileSource, TILE_URL};
use crate::gps;
use crate::timelapse::ffmpeg::ffmpeg_command;
use crate::timelapse::CancelFlag;

#[derive(Debug)]
pub struct ExportOutcome {
    pub output_path: PathBuf,
    pub warnings: Vec<String>,
}

fn err(msg: impl Into<String>) -> AppError {
    AppError::Internal(msg.into())
}

/// Force an `.mp4` extension: the muxer is picked from it.
pub fn normalize_output_path(p: &str) -> PathBuf {
    let path = PathBuf::from(p);
    match path.extension().and_then(|e| e.to_str()) {
        Some(e) if e.eq_ignore_ascii_case("mp4") => path,
        _ => {
            let mut s = path.into_os_string();
            s.push(".mp4");
            PathBuf::from(s)
        }
    }
}

fn partial_path(output: &Path) -> PathBuf {
    let name = output
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "export.mp4".into());
    output.with_file_name(format!(".{name}.partial.mp4"))
}

/// GPS points for the clip, in clip time. Points outside the clip (plus a
/// second of slack so the dot interpolates cleanly at the edges) are
/// dropped.
fn gather_track(args: &StartExportArgs, prepared: &Prepared) -> Vec<TrackPoint> {
    let mut pts: Vec<TrackPoint> = Vec::new();
    for src in &prepared.gps_sources {
        let points =
            gps::extract_for_kind(Path::new(&src.path), args.camera_kind).unwrap_or_default();
        for p in points {
            if !p.fix_ok || p.t_offset_s < 0.0 || p.t_offset_s > src.duration_s {
                continue;
            }
            let t = src.offset_s + p.t_offset_s;
            if t >= -1.0 && t <= prepared.duration_s + 1.0 {
                pts.push(TrackPoint {
                    t,
                    lat: p.lat,
                    lon: p.lon,
                });
            }
        }
    }
    pts.sort_by(|a, b| a.t.total_cmp(&b.t));
    pts
}

/// Street-map picture for the track, or why there isn't one.
fn build_backdrop(
    track: &[TrackPoint],
    w: u32,
    h: u32,
    source: &dyn TileSource,
) -> Result<Backdrop, String> {
    let latlon: Vec<(f64, f64)> = track.iter().map(|p| (p.lat, p.lon)).collect();
    let view = choose_view(&latlon, w, h).ok_or("no GPS points")?;
    let rgb = render_backdrop(&view, w, h, source)?;
    Ok(Backdrop { rgb, view })
}

pub fn run_export(
    args: &StartExportArgs,
    ffmpeg_path: &str,
    cancel: &CancelFlag,
    on_progress: Arc<dyn Fn(f64) + Send + Sync>,
) -> Result<ExportOutcome, AppError> {
    let mut prepared = prepare(args)?;
    let output = normalize_output_path(&args.output_path);

    // Never write over a source recording.
    for seg in &args.segments {
        for f in &seg.files {
            if Path::new(&f.path) == output {
                return Err(err("the output file would overwrite a source recording"));
            }
        }
    }
    if let Some(parent) = output.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }

    let scratch = std::env::temp_dir().join(format!("tripviewer-export-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&scratch)?;
    let partial = partial_path(&output);

    let result = run_inner(
        args,
        &mut prepared,
        ffmpeg_path,
        cancel,
        on_progress,
        &scratch,
        &partial,
    );
    let _ = fs::remove_dir_all(&scratch);

    match result {
        Ok(()) => match fs::rename(&partial, &output) {
            Ok(()) => Ok(ExportOutcome {
                output_path: output,
                warnings: prepared.warnings,
            }),
            Err(e) => {
                let _ = fs::remove_file(&partial);
                Err(err(format!("could not move the export into place: {e}")))
            }
        },
        Err(e) => {
            let _ = fs::remove_file(&partial);
            Err(e)
        }
    }
}

fn run_inner(
    args: &StartExportArgs,
    prepared: &mut Prepared,
    ffmpeg_path: &str,
    cancel: &CancelFlag,
    on_progress: Arc<dyn Fn(f64) + Send + Sync>,
    scratch: &Path,
    partial: &Path,
) -> Result<(), AppError> {
    let mut lists: Vec<PathBuf> = Vec::new();
    for (i, text) in prepared.list_texts.iter().enumerate() {
        let p = scratch.join(format!("channel{i}.txt"));
        fs::write(&p, text)?;
        lists.push(p);
    }

    // Mini-map: render to a raw file that ffmpeg reads as a 1 fps video.
    let mut map_raw: Option<(PathBuf, u32, u32)> = None;
    if args.show_map {
        let track = gather_track(args, prepared);
        if track.is_empty() {
            prepared
                .warnings
                .push("Map skipped: no GPS data in this clip".into());
        } else {
            let (w, h) = map_tile_size(
                args.layout,
                args.slot_size,
                args.pip_edge,
                prepared.slots.len(),
            );
            let backdrop = if args.map_style == MapStyle::Streets {
                match build_backdrop(&track, w, h, &HttpTiles::new(TILE_URL)) {
                    Ok(b) => Some(b),
                    Err(e) => {
                        prepared
                            .warnings
                            .push(format!("Street map unavailable ({e}): drew the route only"));
                        None
                    }
                }
            } else {
                None
            };
            let path = scratch.join("map.raw");
            let mut out = BufWriter::new(fs::File::create(&path)?);
            render_frames(
                &track,
                w,
                h,
                prepared.duration_s,
                backdrop.as_ref(),
                &mut out,
            )?;
            std::io::Write::flush(&mut out)?;
            map_raw = Some((path, w, h));
        }
    }

    let spec = ExportSpec {
        layout: args.layout,
        slot_size: args.slot_size,
        pip_edge: args.pip_edge,
        quality: args.quality,
        codec: args.codec,
        trim_start_s: prepared.local_start_s,
        duration_s: prepared.duration_s,
        channel_lists: &lists,
        main: prepared.main,
        slots: &prepared.slots,
        map: map_raw.as_ref().map(|(p, w, h)| (p.as_path(), *w, *h)),
        audio: &prepared.audio,
        output: partial,
    };
    let ff_args = build_args(&spec);

    let mut cmd = ffmpeg_command(ffmpeg_path);
    cmd.args(["-y", "-hide_banner", "-nostats", "-loglevel", "error"])
        .args(["-progress", "pipe:1"])
        .args(&ff_args);

    let cb = on_progress.clone();
    run_with_progress(cmd, prepared.duration_s, cancel, move |f| cb(f))?;
    on_progress(1.0);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_path_gets_an_mp4_extension() {
        assert_eq!(normalize_output_path("/a/b.mp4"), PathBuf::from("/a/b.mp4"));
        assert_eq!(normalize_output_path("/a/b.MP4"), PathBuf::from("/a/b.MP4"));
        assert_eq!(normalize_output_path("/a/b"), PathBuf::from("/a/b.mp4"));
        assert_eq!(
            normalize_output_path("/a/b.mov"),
            PathBuf::from("/a/b.mov.mp4")
        );
    }

    #[test]
    fn backdrop_failure_is_reported_not_fatal() {
        struct Down;
        impl TileSource for Down {
            fn fetch(&self, _: u32, _: u32, _: u32) -> Result<Vec<u8>, String> {
                Err("offline".into())
            }
        }
        let track = vec![TrackPoint {
            t: 0.0,
            lat: 45.0,
            lon: 12.0,
        }];
        assert_eq!(
            build_backdrop(&track, 384, 216, &Down).err().as_deref(),
            Some("offline")
        );
    }

    #[test]
    fn partial_file_is_hidden_next_to_the_output() {
        assert_eq!(
            partial_path(Path::new("/a/clip.mp4")),
            PathBuf::from("/a/.clip.mp4.partial.mp4")
        );
    }
}
