use crate::error::AppError;
use crate::model::ChannelMeta;
use mp4::{MediaType, TrackType, read_mp4};
use std::fs::File;
use std::path::Path;

pub fn probe(path: &Path) -> Result<ChannelMeta, AppError> {
    let file = File::open(path)?;
    // Intentionally *not* prefixing with the path — callers carry the
    // path separately (see `ScanError.path`), and the scan classifier
    // substring-matches the raw mp4-crate message to pick a category.
    let mp4 = read_mp4(file).map_err(|e| AppError::Parse(e.to_string()))?;

    let tracks = mp4.tracks();

    let video_track = tracks
        .values()
        .find(|t| matches!(t.track_type(), Ok(TrackType::Video)))
        .ok_or_else(|| AppError::NotVideo(path.to_string_lossy().into_owned()))?;

    let duration_s = video_track.duration().as_secs_f64();
    let width = video_track.width() as u32;
    let height = video_track.height() as u32;

    let fps = video_track.frame_rate();
    let (fps_num, fps_den) = rational_fps(fps);

    let codec = match video_track.media_type() {
        Ok(MediaType::H264) => "h264",
        Ok(MediaType::H265) => "hevc",
        Ok(MediaType::VP9) => "vp9",
        _ => "unknown",
    }
    .to_string();

    let has_gpmd_track = tracks.values().any(|t| {
        let ht = t.trak.mdia.hdlr.handler_type.value;
        ht != *b"vide" && ht != *b"soun" && ht != *b"sbtl" && ht != [0, 0, 0, 0]
    });

    Ok(ChannelMeta {
        duration_s,
        width,
        height,
        fps_num,
        fps_den,
        codec,
        has_gpmd_track,
    })
}

fn rational_fps(fps: f64) -> (u32, u32) {
    if !fps.is_finite() || fps <= 0.0 {
        return (0, 1);
    }
    if (fps - fps.round()).abs() < 0.01 {
        return (fps.round() as u32, 1);
    }
    // NTSC-style 29.97 / 59.94 / 23.976 families
    let ntsc = fps * 1001.0 / 1000.0;
    if (ntsc - ntsc.round()).abs() < 0.01 {
        return (ntsc.round() as u32 * 1000, 1001);
    }
    ((fps * 1000.0).round() as u32, 1000)
}
