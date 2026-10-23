//! GPS mini-map for exported clips: the route as a line on a dark
//! background with a dot at the current position.
//!
//! By default the map is tile-less: the route shape plus a moving dot is
//! enough to read where the car is, works offline and has no licensing
//! strings attached. Optionally a street-map [`Backdrop`] (see
//! `export::tiles`) is drawn underneath; the route and dot then use
//! colours that stand out on a light map.
//!
//! Frames are plain `rgb24`, one per second, streamed into a writer that
//! ffmpeg later reads as `-f rawvideo`.

use std::io::{self, Write};

const BG: [u8; 3] = [20, 23, 26];
const BORDER: [u8; 3] = [52, 58, 64];
const ROUTE_AHEAD: [u8; 3] = [92, 98, 106];
const ROUTE_DONE: [u8; 3] = [236, 236, 236];
const DOT_FILL: [u8; 3] = [255, 90, 54];
const DOT_RING: [u8; 3] = [20, 23, 26];

/// Colours when drawn over a street map.
const MAP_CASING: [u8; 3] = [255, 255, 255];
const MAP_ROUTE_AHEAD: [u8; 3] = [52, 101, 220];
const MAP_ROUTE_DONE: [u8; 3] = [220, 38, 38];
const MAP_DOT_FILL: [u8; 3] = [255, 140, 0];
const MAP_DOT_RING: [u8; 3] = [255, 255, 255];

/// A street-map picture (`w*h*3` RGB bytes) and the view it was cut with.
pub struct Backdrop {
    pub rgb: Vec<u8>,
    pub view: crate::export::tiles::View,
}

const MARGIN: f64 = 28.0;
/// Smallest geographic span (degrees, ~55 m) the view will zoom into, so a
/// parked car or a very short clip shows a dot instead of a blown-up speck.
const MIN_SPAN_DEG: f64 = 0.0005;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrackPoint {
    /// Seconds from the start of the exported clip.
    pub t: f64,
    pub lat: f64,
    pub lon: f64,
}

struct Canvas {
    w: usize,
    h: usize,
    px: Vec<u8>,
}

impl Canvas {
    fn new(w: u32, h: u32) -> Self {
        let (w, h) = (w as usize, h as usize);
        let mut px = vec![0u8; w * h * 3];
        for chunk in px.chunks_exact_mut(3) {
            chunk.copy_from_slice(&BG);
        }
        let mut c = Canvas { w, h, px };
        c.border();
        c
    }

    fn set(&mut self, x: i32, y: i32, rgb: [u8; 3]) {
        if x < 0 || y < 0 || x as usize >= self.w || y as usize >= self.h {
            return;
        }
        let i = (y as usize * self.w + x as usize) * 3;
        self.px[i..i + 3].copy_from_slice(&rgb);
    }

    fn border(&mut self) {
        for x in 0..self.w as i32 {
            self.set(x, 0, BORDER);
            self.set(x, self.h as i32 - 1, BORDER);
        }
        for y in 0..self.h as i32 {
            self.set(0, y, BORDER);
            self.set(self.w as i32 - 1, y, BORDER);
        }
    }

    fn disc(&mut self, cx: i32, cy: i32, r: i32, rgb: [u8; 3]) {
        for dy in -r..=r {
            for dx in -r..=r {
                if dx * dx + dy * dy <= r * r {
                    self.set(cx + dx, cy + dy, rgb);
                }
            }
        }
    }

    /// Bresenham with a round brush for line thickness.
    fn line(&mut self, a: (i32, i32), b: (i32, i32), r: i32, rgb: [u8; 3]) {
        let (mut x0, mut y0) = a;
        let (x1, y1) = b;
        let dx = (x1 - x0).abs();
        let dy = -(y1 - y0).abs();
        let sx = if x0 < x1 { 1 } else { -1 };
        let sy = if y0 < y1 { 1 } else { -1 };
        let mut err = dx + dy;
        loop {
            self.disc(x0, y0, r, rgb);
            if x0 == x1 && y0 == y1 {
                break;
            }
            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                x0 += sx;
            }
            if e2 <= dx {
                err += dx;
                y0 += sy;
            }
        }
    }
}

/// Maps lat/lon to pixel coordinates, keeping the real aspect ratio.
struct Projection {
    lat0: f64,
    lon0: f64,
    kx: f64,
    scale: f64,
    off_x: f64,
    off_y: f64,
}

impl Projection {
    fn fit(points: &[TrackPoint], w: u32, h: u32) -> Self {
        let (mut min_lat, mut max_lat) = (f64::MAX, f64::MIN);
        let (mut min_lon, mut max_lon) = (f64::MAX, f64::MIN);
        for p in points {
            min_lat = min_lat.min(p.lat);
            max_lat = max_lat.max(p.lat);
            min_lon = min_lon.min(p.lon);
            max_lon = max_lon.max(p.lon);
        }
        let mid_lat = (min_lat + max_lat) / 2.0;
        let mid_lon = (min_lon + max_lon) / 2.0;
        // Longitude degrees shrink with latitude.
        let kx = mid_lat.to_radians().cos().max(0.01);
        let span_x = ((max_lon - min_lon) * kx).max(MIN_SPAN_DEG);
        let span_y = (max_lat - min_lat).max(MIN_SPAN_DEG);
        let avail_w = (w as f64 - 2.0 * MARGIN).max(1.0);
        let avail_h = (h as f64 - 2.0 * MARGIN).max(1.0);
        let scale = (avail_w / span_x).min(avail_h / span_y);
        Projection {
            lat0: mid_lat,
            lon0: mid_lon,
            kx,
            scale,
            off_x: w as f64 / 2.0,
            off_y: h as f64 / 2.0,
        }
    }

    fn to_px(&self, lat: f64, lon: f64) -> (i32, i32) {
        let x = self.off_x + (lon - self.lon0) * self.kx * self.scale;
        let y = self.off_y - (lat - self.lat0) * self.scale;
        (x.round() as i32, y.round() as i32)
    }
}

/// Position at time `t`, interpolated between neighbouring points and
/// clamped to the first/last point outside the covered range.
pub fn position_at(points: &[TrackPoint], t: f64) -> Option<(f64, f64)> {
    let first = points.first()?;
    let last = points.last()?;
    if t <= first.t {
        return Some((first.lat, first.lon));
    }
    if t >= last.t {
        return Some((last.lat, last.lon));
    }
    let i = points.partition_point(|p| p.t <= t);
    let (a, b) = (points[i - 1], points[i]);
    let span = b.t - a.t;
    let f = if span > 0.0 { (t - a.t) / span } else { 0.0 };
    Some((a.lat + (b.lat - a.lat) * f, a.lon + (b.lon - a.lon) * f))
}

/// Render one frame per second for `duration_s` seconds and hand each
/// `w*h*3`-byte frame to `sink`. Returns the number of frames written.
/// `points` must be sorted by `t`; with no points nothing is written.
pub fn render_frames<W: Write>(
    points: &[TrackPoint],
    w: u32,
    h: u32,
    duration_s: f64,
    backdrop: Option<&Backdrop>,
    sink: &mut W,
) -> io::Result<usize> {
    if points.is_empty() || w < 16 || h < 16 {
        return Ok(0);
    }
    let backdrop = backdrop.filter(|b| b.rgb.len() == (w * h * 3) as usize);
    let flat = Projection::fit(points, w, h);
    let to_px = |lat: f64, lon: f64| match backdrop {
        Some(b) => b.view.to_px(lat, lon),
        None => flat.to_px(lat, lon),
    };
    let (c_ahead, c_done, c_fill, c_ring) = if backdrop.is_some() {
        (MAP_ROUTE_AHEAD, MAP_ROUTE_DONE, MAP_DOT_FILL, MAP_DOT_RING)
    } else {
        (ROUTE_AHEAD, ROUTE_DONE, DOT_FILL, DOT_RING)
    };
    let px: Vec<(i32, i32)> = points.iter().map(|p| to_px(p.lat, p.lon)).collect();

    // The full route in the "still to go" colour is the same in every
    // frame, so draw it once and copy it per frame.
    let mut base = Canvas::new(w, h);
    if let Some(b) = backdrop {
        base.px.copy_from_slice(&b.rgb);
        // A white casing keeps the line readable on busy map areas.
        for pair in px.windows(2) {
            base.line(pair[0], pair[1], 4, MAP_CASING);
        }
    }
    for pair in px.windows(2) {
        base.line(pair[0], pair[1], 2, c_ahead);
    }
    if px.len() == 1 {
        base.disc(px[0].0, px[0].1, 2, c_ahead);
    }

    let frames = duration_s.max(0.0).ceil() as usize;
    for k in 0..frames.max(1) {
        let t = k as f64;
        let mut frame = Canvas {
            w: base.w,
            h: base.h,
            px: base.px.clone(),
        };
        // Route already driven: every point up to now, then up to the
        // interpolated current position.
        let done = points.partition_point(|p| p.t <= t);
        for i in 1..done {
            frame.line(px[i - 1], px[i], 2, c_done);
        }
        let (lat, lon) = position_at(points, t).expect("non-empty");
        let cur = to_px(lat, lon);
        if done >= 1 {
            frame.line(px[done - 1], cur, 2, c_done);
        }
        frame.disc(cur.0, cur.1, 9, c_ring);
        frame.disc(cur.0, cur.1, 6, c_fill);
        sink.write_all(&frame.px)?;
    }
    Ok(frames.max(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pt(t: f64, lat: f64, lon: f64) -> TrackPoint {
        TrackPoint { t, lat, lon }
    }

    // Invented coordinates, deliberately not anyone's real route.
    fn route() -> Vec<TrackPoint> {
        vec![
            pt(0.0, 45.0000, 12.0000),
            pt(1.0, 45.0002, 12.0002),
            pt(2.0, 45.0004, 12.0004),
            pt(3.0, 45.0004, 12.0008),
        ]
    }

    #[test]
    fn position_interpolates_between_points() {
        let r = route();
        let (lat, lon) = position_at(&r, 0.5).unwrap();
        assert!((lat - 45.0001).abs() < 1e-9);
        assert!((lon - 12.0001).abs() < 1e-9);
    }

    #[test]
    fn position_clamps_outside_the_range() {
        let r = route();
        assert_eq!(position_at(&r, -5.0), Some((45.0000, 12.0000)));
        assert_eq!(position_at(&r, 99.0), Some((45.0004, 12.0008)));
        assert_eq!(position_at(&[], 1.0), None);
    }

    #[test]
    fn frame_count_and_size_match_the_request() {
        let mut buf: Vec<u8> = Vec::new();
        let n = render_frames(&route(), 160, 90, 3.0, None, &mut buf).unwrap();
        assert_eq!(n, 3);
        assert_eq!(buf.len(), 3 * 160 * 90 * 3);
    }

    #[test]
    fn partial_second_rounds_up() {
        let mut buf = Vec::new();
        let n = render_frames(&route(), 64, 36, 2.2, None, &mut buf).unwrap();
        assert_eq!(n, 3);
    }

    #[test]
    fn no_points_writes_nothing() {
        let mut buf = Vec::new();
        assert_eq!(render_frames(&[], 160, 90, 5.0, None, &mut buf).unwrap(), 0);
        assert!(buf.is_empty());
    }

    fn has_colour(frame: &[u8], rgb: [u8; 3]) -> bool {
        frame.chunks_exact(3).any(|p| p == rgb)
    }

    #[test]
    fn frames_contain_dot_and_route_and_the_dot_moves() {
        let mut buf = Vec::new();
        render_frames(&route(), 160, 90, 3.0, None, &mut buf).unwrap();
        let fsz = 160 * 90 * 3;
        let (f0, f2) = (&buf[0..fsz], &buf[2 * fsz..3 * fsz]);
        assert!(has_colour(f0, DOT_FILL));
        assert!(has_colour(f0, ROUTE_AHEAD));
        // At t=0 nothing has been driven yet; by t=2 some route is done.
        assert!(!has_colour(f0, ROUTE_DONE));
        assert!(has_colour(f2, ROUTE_DONE));
        assert_ne!(f0, f2);
    }

    #[test]
    fn single_point_and_parked_car_do_not_panic() {
        let mut buf = Vec::new();
        let one = vec![pt(0.0, 45.0, 12.0)];
        assert_eq!(
            render_frames(&one, 160, 90, 2.0, None, &mut buf).unwrap(),
            2
        );
        let parked = vec![pt(0.0, 45.0, 12.0), pt(1.0, 45.0, 12.0)];
        buf.clear();
        assert_eq!(
            render_frames(&parked, 160, 90, 2.0, None, &mut buf).unwrap(),
            2
        );
    }

    #[test]
    fn route_stays_inside_the_frame_margin() {
        let proj = Projection::fit(&route(), 640, 360);
        for p in route() {
            let (x, y) = proj.to_px(p.lat, p.lon);
            assert!((27..=613).contains(&x), "x={x}");
            assert!((27..=333).contains(&y), "y={y}");
        }
    }

    #[test]
    fn a_backdrop_is_kept_and_drawn_over() {
        use crate::export::tiles::{choose_view, View};
        let pts = route();
        let latlon: Vec<(f64, f64)> = pts.iter().map(|p| (p.lat, p.lon)).collect();
        let (w, h) = (160u32, 90u32);
        let view: View = choose_view(&latlon, w, h).unwrap();
        let bd = Backdrop {
            rgb: vec![200u8; (w * h * 3) as usize],
            view,
        };
        let mut buf = Vec::new();
        render_frames(&pts, w, h, 2.0, Some(&bd), &mut buf).unwrap();
        let f0 = &buf[..(w * h * 3) as usize];
        // The map shows through, the route and dot use the map palette.
        assert!(has_colour(f0, [200, 200, 200]));
        assert!(has_colour(f0, MAP_ROUTE_AHEAD));
        assert!(has_colour(f0, MAP_DOT_FILL));
        assert!(!has_colour(f0, BG));
    }

    #[test]
    fn a_backdrop_of_the_wrong_size_is_ignored() {
        use crate::export::tiles::{choose_view, View};
        let pts = route();
        let latlon: Vec<(f64, f64)> = pts.iter().map(|p| (p.lat, p.lon)).collect();
        let view: View = choose_view(&latlon, 160, 90).unwrap();
        let bd = Backdrop {
            rgb: vec![0u8; 10],
            view,
        };
        let mut buf = Vec::new();
        assert_eq!(
            render_frames(&pts, 160, 90, 1.0, Some(&bd), &mut buf).unwrap(),
            1
        );
        assert!(buf.chunks_exact(3).any(|p| p == BG));
    }
}
