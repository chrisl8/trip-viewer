//! Street-map backdrop for the exported mini-map.
//!
//! One still image is built per export: the handful of map tiles that
//! cover the clip's route (at most [`MAX_TILES`], roughly what the map
//! pane in the app loads when you look at a trip) are fetched once,
//! stitched together and kept only in the export's scratch folder. Nothing
//! is cached between exports and nothing is prefetched, which keeps this
//! inside the OpenStreetMap tile usage policy; the policy's attribution
//! requirement is met by burning "(c) OpenStreetMap contributors" into
//! the picture.
//!
//! Everything but [`HttpTiles`] is pure and testable without a network:
//! the fetching is behind the [`TileSource`] trait.

use std::io::{Cursor, Read as _};

pub const TILE_URL: &str = "https://tile.openstreetmap.org/{z}/{x}/{y}.png";
pub const USER_AGENT: &str = concat!(
    "TripViewer/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/chrisl8/trip-viewer)"
);

pub const TILE_PX: u32 = 256;
/// Hard ceiling on requests per export. A 640x360 map needs at most 12.
pub const MAX_TILES: usize = 25;
const MIN_ZOOM: u32 = 3;
const MAX_ZOOM: u32 = 17;
/// Keep the route this far from the edges of the map (pixels).
const MARGIN: f64 = 28.0;

/// Where the map window sits in "world pixel" space at a given zoom
/// (Web Mercator, 256 px tiles, origin at the top-left of the world).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct View {
    pub zoom: u32,
    /// World pixel at the top-left corner of the map image.
    pub origin_x: f64,
    pub origin_y: f64,
}

impl View {
    pub fn to_px(&self, lat: f64, lon: f64) -> (i32, i32) {
        let (wx, wy) = world_px(lat, lon, self.zoom);
        (
            (wx - self.origin_x).round() as i32,
            (wy - self.origin_y).round() as i32,
        )
    }
}

pub fn world_px(lat: f64, lon: f64, zoom: u32) -> (f64, f64) {
    let size = (TILE_PX as f64) * f64::from(1u32 << zoom);
    let lat = lat.clamp(-85.0511, 85.0511).to_radians();
    let x = (lon + 180.0) / 360.0 * size;
    let y = (1.0 - (lat.tan() + 1.0 / lat.cos()).ln() / std::f64::consts::PI) / 2.0 * size;
    (x, y)
}

/// Pick the closest zoom at which the whole route fits in a `w` x `h` map
/// with a margin, and centre the window on the route.
pub fn choose_view(points: &[(f64, f64)], w: u32, h: u32) -> Option<View> {
    if points.is_empty() {
        return None;
    }
    let avail_w = (f64::from(w) - 2.0 * MARGIN).max(1.0);
    let avail_h = (f64::from(h) - 2.0 * MARGIN).max(1.0);
    for zoom in (MIN_ZOOM..=MAX_ZOOM).rev() {
        let (mut min_x, mut max_x) = (f64::MAX, f64::MIN);
        let (mut min_y, mut max_y) = (f64::MAX, f64::MIN);
        for &(lat, lon) in points {
            let (x, y) = world_px(lat, lon, zoom);
            min_x = min_x.min(x);
            max_x = max_x.max(x);
            min_y = min_y.min(y);
            max_y = max_y.max(y);
        }
        if (max_x - min_x) <= avail_w && (max_y - min_y) <= avail_h || zoom == MIN_ZOOM {
            return Some(View {
                zoom,
                origin_x: (min_x + max_x) / 2.0 - f64::from(w) / 2.0,
                origin_y: (min_y + max_y) / 2.0 - f64::from(h) / 2.0,
            });
        }
    }
    None
}

/// Tile coordinates covering the map window, row by row.
pub fn tiles_needed(view: &View, w: u32, h: u32) -> Vec<(u32, u32)> {
    let n = 1i64 << view.zoom;
    let t = f64::from(TILE_PX);
    let x0 = (view.origin_x / t).floor() as i64;
    let x1 = ((view.origin_x + f64::from(w) - 1.0) / t).floor() as i64;
    let y0 = (view.origin_y / t).floor() as i64;
    let y1 = ((view.origin_y + f64::from(h) - 1.0) / t).floor() as i64;
    let mut out = Vec::new();
    for ty in y0..=y1 {
        if ty < 0 || ty >= n {
            continue;
        }
        for tx in x0..=x1 {
            out.push((tx.rem_euclid(n) as u32, ty as u32));
        }
    }
    out
}

pub trait TileSource {
    /// PNG bytes of one tile.
    fn fetch(&self, zoom: u32, x: u32, y: u32) -> Result<Vec<u8>, String>;
}

pub struct HttpTiles {
    url_template: String,
    agent: ureq::Agent,
}

impl HttpTiles {
    pub fn new(url_template: &str) -> Self {
        HttpTiles {
            url_template: url_template.to_string(),
            agent: ureq::AgentBuilder::new()
                .timeout(std::time::Duration::from_secs(10))
                .user_agent(USER_AGENT)
                .build(),
        }
    }
}

impl TileSource for HttpTiles {
    fn fetch(&self, zoom: u32, x: u32, y: u32) -> Result<Vec<u8>, String> {
        let url = self
            .url_template
            .replace("{z}", &zoom.to_string())
            .replace("{x}", &x.to_string())
            .replace("{y}", &y.to_string());
        let resp = self.agent.get(&url).call().map_err(|e| e.to_string())?;
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut resp.into_reader().take(2_000_000), &mut bytes)
            .map_err(|e| e.to_string())?;
        Ok(bytes)
    }
}

/// Decode a PNG tile to tightly packed RGB (any colour type, alpha dropped).
pub fn decode_tile(png_bytes: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
    let mut dec = png::Decoder::new(Cursor::new(png_bytes));
    dec.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = dec.read_info().map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).map_err(|e| e.to_string())?;
    let (w, h) = (info.width, info.height);
    let data = &buf[..info.buffer_size()];
    let px = (w * h) as usize;
    let mut rgb = Vec::with_capacity(px * 3);
    match info.color_type {
        png::ColorType::Rgb => rgb.extend_from_slice(data),
        png::ColorType::Rgba => {
            for c in data.chunks_exact(4) {
                rgb.extend_from_slice(&c[..3]);
            }
        }
        png::ColorType::Grayscale => {
            for &g in data {
                rgb.extend_from_slice(&[g, g, g]);
            }
        }
        png::ColorType::GrayscaleAlpha => {
            for c in data.chunks_exact(2) {
                rgb.extend_from_slice(&[c[0], c[0], c[0]]);
            }
        }
        png::ColorType::Indexed => return Err("unexpected indexed PNG".into()),
    }
    if rgb.len() != px * 3 {
        return Err("tile size mismatch".into());
    }
    Ok((w, h, rgb))
}

/// Fetch and stitch the tiles into a `w` x `h` RGB image for `view`.
pub fn render_backdrop(
    view: &View,
    w: u32,
    h: u32,
    source: &dyn TileSource,
) -> Result<Vec<u8>, String> {
    let tiles = tiles_needed(view, w, h);
    if tiles.is_empty() {
        return Err("no map tiles cover this area".into());
    }
    if tiles.len() > MAX_TILES {
        return Err(format!("map would need {} tiles", tiles.len()));
    }
    let mut img = vec![225u8; (w * h * 3) as usize];
    let n = 1i64 << view.zoom;
    let t = f64::from(TILE_PX);
    let x_first = (view.origin_x / t).floor() as i64;
    for &(tx, ty) in &tiles {
        let bytes = source.fetch(view.zoom, tx, ty)?;
        let (tw, th, rgb) = decode_tile(&bytes)?;
        // Which copy of the (possibly wrapped) column this is.
        let mut col = x_first;
        while col.rem_euclid(n) != i64::from(tx) {
            col += 1;
        }
        let dest_x = (col as f64 * t - view.origin_x).round() as i64;
        let dest_y = (f64::from(ty) * t - view.origin_y).round() as i64;
        for row in 0..th as i64 {
            let y = dest_y + row;
            if y < 0 || y >= i64::from(h) {
                continue;
            }
            for cx in 0..tw as i64 {
                let x = dest_x + cx;
                if x < 0 || x >= i64::from(w) {
                    continue;
                }
                let s = ((row * i64::from(tw) + cx) * 3) as usize;
                let d = ((y * i64::from(w) + x) * 3) as usize;
                img[d..d + 3].copy_from_slice(&rgb[s..s + 3]);
            }
        }
    }
    burn_attribution(&mut img, w, h);
    Ok(img)
}

// --- attribution text -----------------------------------------------------

const GLYPH_W: usize = 5;
const GLYPH_H: usize = 7;

fn glyph(c: char) -> Option<[&'static str; GLYPH_H]> {
    Some(match c {
        'A' => [
            ".XXX.", "X...X", "X...X", "XXXXX", "X...X", "X...X", "X...X",
        ],
        'B' => [
            "XXXX.", "X...X", "X...X", "XXXX.", "X...X", "X...X", "XXXX.",
        ],
        'C' => [
            ".XXX.", "X...X", "X....", "X....", "X....", "X...X", ".XXX.",
        ],
        'E' => [
            "XXXXX", "X....", "X....", "XXXX.", "X....", "X....", "XXXXX",
        ],
        'I' => [
            "XXXXX", "..X..", "..X..", "..X..", "..X..", "..X..", "XXXXX",
        ],
        'M' => [
            "X...X", "XX.XX", "X.X.X", "X.X.X", "X...X", "X...X", "X...X",
        ],
        'N' => [
            "X...X", "XX..X", "X.X.X", "X..XX", "X...X", "X...X", "X...X",
        ],
        'O' => [
            ".XXX.", "X...X", "X...X", "X...X", "X...X", "X...X", ".XXX.",
        ],
        'P' => [
            "XXXX.", "X...X", "X...X", "XXXX.", "X....", "X....", "X....",
        ],
        'R' => [
            "XXXX.", "X...X", "X...X", "XXXX.", "X.X..", "X..X.", "X...X",
        ],
        'S' => [
            ".XXXX", "X....", "X....", ".XXX.", "....X", "....X", "XXXX.",
        ],
        'T' => [
            "XXXXX", "..X..", "..X..", "..X..", "..X..", "..X..", "..X..",
        ],
        'U' => [
            "X...X", "X...X", "X...X", "X...X", "X...X", "X...X", ".XXX.",
        ],
        ' ' => [
            ".....", ".....", ".....", ".....", ".....", ".....", ".....",
        ],
        _ => return None,
    })
}

const COPYRIGHT: [&str; 7] = [
    ".XXXXX.", "X.....X", "X.XXX.X", "X.X...X", "X.XXX.X", "X.....X", ".XXXXX.",
];

/// "(c) OPENSTREETMAP CONTRIBUTORS" in the bottom-right corner, dark text
/// on a light box so it reads on any map colour.
pub fn burn_attribution(img: &mut [u8], w: u32, h: u32) {
    let text = "OPENSTREETMAP CONTRIBUTORS";
    let scale: usize = if w >= 480 { 2 } else { 1 };
    let pad = 3 * scale;
    let text_w = (7 + 2 + text.chars().count() * (GLYPH_W + 1)) * scale;
    let box_w = text_w + 2 * pad;
    let box_h = GLYPH_H * scale + 2 * pad;
    let (w, h) = (w as usize, h as usize);
    if box_w >= w || box_h >= h {
        return;
    }
    let x0 = w - box_w - 1;
    let y0 = h - box_h - 1;
    let mut put = |x: usize, y: usize, rgb: [u8; 3]| {
        if x < w && y < h {
            let i = (y * w + x) * 3;
            img[i..i + 3].copy_from_slice(&rgb);
        }
    };
    for y in y0..y0 + box_h {
        for x in x0..x0 + box_w {
            put(x, y, [248, 248, 248]);
        }
    }
    let ink = [40, 40, 40];
    let mut draw = |rows: &[&str], ox: usize, oy: usize| {
        for (ry, row) in rows.iter().enumerate() {
            for (rx, ch) in row.chars().enumerate() {
                if ch == 'X' {
                    for sy in 0..scale {
                        for sx in 0..scale {
                            put(ox + rx * scale + sx, oy + ry * scale + sy, ink);
                        }
                    }
                }
            }
        }
    };
    let ty = y0 + pad;
    let mut cx = x0 + pad;
    draw(&COPYRIGHT, cx, ty);
    cx += (7 + 2) * scale;
    for ch in text.chars() {
        if let Some(g) = glyph(ch) {
            draw(&g, cx, ty);
        }
        cx += (GLYPH_W + 1) * scale;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// A valid PNG filled with one colour.
    fn solid_png(rgb: [u8; 3]) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut out, TILE_PX, TILE_PX);
            enc.set_color(png::ColorType::Rgb);
            enc.set_depth(png::BitDepth::Eight);
            let mut w = enc.write_header().unwrap();
            let data: Vec<u8> = (0..TILE_PX * TILE_PX).flat_map(|_| rgb).collect();
            w.write_image_data(&data).unwrap();
        }
        out
    }

    struct Fake {
        calls: RefCell<Vec<(u32, u32, u32)>>,
    }
    impl TileSource for Fake {
        fn fetch(&self, z: u32, x: u32, y: u32) -> Result<Vec<u8>, String> {
            self.calls.borrow_mut().push((z, x, y));
            // Checkerboard of two colours so stitching errors are visible.
            Ok(solid_png(if (x + y).is_multiple_of(2) {
                [200, 220, 200]
            } else {
                [220, 200, 200]
            }))
        }
    }

    // Invented coordinates.
    fn route() -> Vec<(f64, f64)> {
        vec![(45.0000, 12.0000), (45.0010, 12.0015), (45.0020, 12.0030)]
    }

    #[test]
    fn world_px_known_values() {
        let (x, y) = world_px(0.0, 0.0, 1);
        assert!((x - 256.0).abs() < 1e-6 && (y - 256.0).abs() < 1e-6);
        let (x, _) = world_px(0.0, -180.0, 4);
        assert!(x.abs() < 1e-6);
    }

    #[test]
    fn view_fits_the_route_inside_the_margins() {
        let (w, h) = (512, 288);
        let v = choose_view(&route(), w, h).unwrap();
        for (lat, lon) in route() {
            let (x, y) = v.to_px(lat, lon);
            assert!((27..=(w as i32 - 27)).contains(&x), "x={x}");
            assert!((27..=(h as i32 - 27)).contains(&y), "y={y}");
        }
        // And it is the closest such zoom: one more would not fit.
        assert!(v.zoom <= MAX_ZOOM);
    }

    #[test]
    fn parked_car_uses_the_closest_zoom() {
        let v = choose_view(&[(45.0, 12.0), (45.0, 12.0)], 384, 216).unwrap();
        assert_eq!(v.zoom, MAX_ZOOM);
        assert_eq!(choose_view(&[], 384, 216), None);
    }

    #[test]
    fn tile_count_is_bounded_for_every_map_size() {
        for (w, h) in [(384, 216), (512, 288), (640, 360), (640, 540)] {
            let v = choose_view(&route(), w, h).unwrap();
            let n = tiles_needed(&v, w, h).len();
            assert!(n > 0 && n <= MAX_TILES, "{w}x{h} needs {n}");
        }
    }

    #[test]
    fn backdrop_is_stitched_from_the_right_tiles() {
        let (w, h) = (512, 288);
        let v = choose_view(&route(), w, h).unwrap();
        let src = Fake {
            calls: RefCell::new(vec![]),
        };
        let img = render_backdrop(&v, w, h, &src).unwrap();
        assert_eq!(img.len(), (w * h * 3) as usize);
        let calls = src.calls.borrow();
        assert_eq!(calls.len(), tiles_needed(&v, w, h).len());
        assert!(calls.iter().all(|c| c.0 == v.zoom));
        // The top-left pixel belongs to the first tile requested.
        let (_, tx, ty) = calls[0];
        let want: [u8; 3] = if (tx + ty).is_multiple_of(2) {
            [200, 220, 200]
        } else {
            [220, 200, 200]
        };
        assert_eq!(&img[0..3], &want);
        // The attribution box is light and sits in the bottom-right corner.
        let i = (((h - 6) * w + (w - 6)) * 3) as usize;
        assert_eq!(&img[i..i + 3], &[248, 248, 248]);
    }

    #[test]
    fn a_failed_tile_fails_the_backdrop() {
        struct Down;
        impl TileSource for Down {
            fn fetch(&self, _: u32, _: u32, _: u32) -> Result<Vec<u8>, String> {
                Err("offline".into())
            }
        }
        let v = choose_view(&route(), 384, 216).unwrap();
        assert!(render_backdrop(&v, 384, 216, &Down).is_err());
    }

    #[test]
    fn garbage_is_not_a_tile() {
        assert!(decode_tile(b"not a png").is_err());
    }

    #[test]
    fn tiny_maps_skip_the_attribution_instead_of_panicking() {
        let mut img = vec![0u8; 40 * 30 * 3];
        burn_attribution(&mut img, 40, 30);
    }
}
