//! File-level parking location, not an inferred GPS-to-frame time mapping.
//!
//! P identifies parking, not its capture rate. Until a media-time association
//! is proven, expose one representative observed position instead of fitting
//! GPS wall time into playback duration or silently trimming most fixes.

use super::TimedGpsPoint;
use std::path::Path;

pub(crate) fn is_parking(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .and_then(crate::scan::viofo::parse)
        // The validated parser encodes the explicit P suffix in its group key.
        // This also works in the full fork without changing its camera enum.
        .is_some_and(|name| name.group_key.ends_with("_P"))
}

/// Select an actual validated fix nearest the coordinate-wise median. This
/// reduces isolated endpoint influence without inventing coordinates, a capture
/// factor, a distance threshold or a filename/GPS timezone alignment. It is a
/// representative file location, not proof the vehicle never moved. Original
/// UTC is retained for provenance; playback offset zero is the file anchor.
pub(crate) fn location(points: Vec<TimedGpsPoint>) -> Vec<TimedGpsPoint> {
    if points.is_empty() {
        return points;
    }
    let mut latitudes: Vec<f64> = points.iter().map(|item| item.point.lat).collect();
    let mut longitudes: Vec<f64> = points.iter().map(|item| item.point.lon).collect();
    latitudes.sort_by(f64::total_cmp);
    longitudes.sort_by(f64::total_cmp);
    let latitude = latitudes[latitudes.len() / 2];
    let longitude = longitudes[longitudes.len() / 2];
    let longitude_weight = latitude.to_radians().cos().powi(2);
    let distance = |item: &TimedGpsPoint| {
        (item.point.lat - latitude).powi(2)
            + longitude_weight * (item.point.lon - longitude).powi(2)
    };
    let mut selected = points
        .into_iter()
        .min_by(|left, right| distance(left).total_cmp(&distance(right)))
        .expect("nonempty validated parking observations");
    selected.point.t_offset_s = 0.0;
    // The shared UI uses zero for unavailable instantaneous speed/heading.
    // Neither value can be associated with a frame without a proven time map.
    selected.point.speed_mps = 0.0;
    selected.point.heading_deg = 0.0;
    vec![selected]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::GpsPoint;
    use chrono::NaiveDate;

    fn point(second: i64, latitude: f64) -> TimedGpsPoint {
        TimedGpsPoint {
            utc: NaiveDate::from_ymd_opt(2020, 1, 1)
                .unwrap()
                .and_hms_opt(0, 0, 0)
                .unwrap()
                + chrono::Duration::seconds(second),
            point: GpsPoint {
                t_offset_s: second as f64,
                lat: latitude,
                lon: 1.0,
                speed_mps: 0.0,
                heading_deg: 0.0,
                altitude_m: 0.0,
                fix_ok: true,
            },
        }
    }

    #[test]
    fn accepts_all_parking_channels_and_counter_generations() {
        for name in [
            "2020_0101_000000_PF.MP4",
            "2020_0101_000000_01PI.MP4",
            "2020_0101_000000_02PR.MP4",
        ] {
            assert!(is_parking(Path::new(name)));
        }
        assert!(!is_parking(Path::new("2020_0101_000000_01F.MP4")));
        assert!(!is_parking(Path::new("unrelated_PF.MP4")));
    }

    #[test]
    fn compressed_wall_time_yields_an_observed_file_location_not_a_scaled_route() {
        let points: Vec<_> = (0..119).map(|i| point(i * 15, 1.0)).collect();
        let location = location(points);
        assert_eq!(location.len(), 1);
        assert_eq!(location[0].point.t_offset_s, 0.0);
        assert_eq!(location[0].point.lat, 1.0);
    }

    #[test]
    fn picks_supported_observation_instead_of_endpoint_or_invented_coordinate() {
        let selected = location(vec![point(0, 1.1), point(15, 1.0), point(30, 1.00001)]);
        assert_eq!(selected[0].point.lat, 1.00001);
        assert_eq!(selected[0].utc, point(30, 1.00001).utc);
        assert!(location(vec![]).is_empty());
    }
}
