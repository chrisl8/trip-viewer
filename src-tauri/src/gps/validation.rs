//! Camera-neutral GPS consistency validation.
//!
//! Camera decoders are responsible only for extracting observations in source
//! order. This module checks whether the individual fields can coexist with
//! neighbouring observations. It intentionally knows nothing about filenames,
//! camera brands, time zones, or geographic regions.
//!
//! The validator is deliberately conservative. `Contradictory` means the
//! available data proves a field cannot participate in the same local sequence.
//! `Unknown` means the data is insufficient or mutually ambiguous; callers may
//! choose not to use that field as a synchronization or route anchor rather than
//! guessing which observation is correct.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldConsistency {
    Unknown,
    Consistent,
    Contradictory,
}

#[derive(Debug, Clone, Copy)]
pub struct GpsObservation {
    /// Physical/source order supplied by the camera-specific decoder.
    pub source_order: usize,
    /// Absolute or relative time on one monotonic clock, in milliseconds.
    pub time_ms: Option<i64>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub speed_mps: Option<f64>,
    pub heading_deg: Option<f64>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ValidationContext {
    /// Maximum possible time span of observations that belong to this source
    /// unit. For a video clip this is derived from the measured clip duration.
    /// No camera-clock or UTC-offset assumption is involved.
    pub max_time_span_ms: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObservationValidation {
    pub time: FieldConsistency,
    pub latitude: FieldConsistency,
    pub longitude: FieldConsistency,
    pub speed: FieldConsistency,
    pub heading: FieldConsistency,
}

impl ObservationValidation {
    /// Trip Viewer can place a point on a playback timeline only when both its
    /// time and complete 2-D position are free of detected contradictions.
    pub fn usable_timed_position(self) -> bool {
        self.time == FieldConsistency::Consistent
            && self.latitude == FieldConsistency::Consistent
            && self.longitude == FieldConsistency::Consistent
    }
}

pub fn validate(
    observations: &[GpsObservation],
    context: ValidationContext,
) -> Vec<ObservationValidation> {
    let mut result: Vec<ObservationValidation> =
        observations.iter().map(initial_validation).collect();

    validate_temporal_consistency(observations, &mut result, context);
    validate_spatial_consistency(observations, &mut result);
    result
}

fn initial_validation(obs: &GpsObservation) -> ObservationValidation {
    ObservationValidation {
        time: if obs.time_ms.is_some() {
            FieldConsistency::Consistent
        } else {
            FieldConsistency::Unknown
        },
        latitude: scalar_status(obs.latitude, |v| (-90.0..=90.0).contains(&v)),
        longitude: scalar_status(obs.longitude, |v| (-180.0..=180.0).contains(&v)),
        speed: scalar_status(obs.speed_mps, |v| v >= 0.0),
        heading: scalar_status(obs.heading_deg, |v| (0.0..360.0).contains(&v)),
    }
}

fn scalar_status(value: Option<f64>, valid: impl FnOnce(f64) -> bool) -> FieldConsistency {
    match value {
        None => FieldConsistency::Unknown,
        Some(v) if v.is_finite() && valid(v) => FieldConsistency::Consistent,
        Some(_) => FieldConsistency::Contradictory,
    }
}

fn validate_temporal_consistency(
    observations: &[GpsObservation],
    result: &mut [ObservationValidation],
    context: ValidationContext,
) {
    // First remove isolated time records that are directly contradicted by the
    // nearest still-consistent observations on both sides. This is an exact
    // ordering constraint: no timezone, cadence, or majority assumption.
    loop {
        let active = time_consistent_indices(result);
        if active.len() < 3 {
            break;
        }

        let mut contradictory = Vec::new();
        for window in active.windows(3) {
            let a = window[0];
            let b = window[1];
            let c = window[2];
            let ta = observations[a].time_ms.unwrap();
            let tb = observations[b].time_ms.unwrap();
            let tc = observations[c].time_ms.unwrap();

            if ta <= tc && (tb < ta || tb > tc) {
                contradictory.push(b);
            }
        }

        if contradictory.is_empty() {
            break;
        }
        contradictory.sort_unstable();
        contradictory.dedup();
        for i in contradictory {
            result[i].time = FieldConsistency::Contradictory;
        }
    }

    // Any remaining backwards edge is ambiguous: at least one endpoint is
    // wrong, but the data does not prove which one. Likewise, when a measured
    // source duration is supplied, two timestamps farther apart than that
    // duration cannot both belong to the same source unit. Mark both endpoints
    // Unknown instead of choosing a winner.
    //
    // Repeat because removing an ambiguous endpoint can expose a new conflict
    // between the observations that become neighbours afterwards.
    loop {
        let active = time_consistent_indices(result);
        if active.len() < 2 {
            break;
        }

        let mut ambiguous = Vec::new();
        for pair in active.windows(2) {
            let a = pair[0];
            let b = pair[1];
            let ta = observations[a].time_ms.unwrap();
            let tb = observations[b].time_ms.unwrap();
            let backwards = ta > tb;
            let span_too_large = context
                .max_time_span_ms
                .filter(|limit| *limit >= 0)
                .map(|limit| ta.abs_diff(tb) > limit as u64)
                .unwrap_or(false);

            if backwards || span_too_large {
                ambiguous.push(a);
                ambiguous.push(b);
            }
        }

        if let (Some(limit), Some(&first), Some(&last)) = (
            context.max_time_span_ms.filter(|limit| *limit >= 0),
            active.first(),
            active.last(),
        ) {
            let first_t = observations[first].time_ms.unwrap();
            let last_t = observations[last].time_ms.unwrap();
            if first_t.abs_diff(last_t) > limit as u64 {
                ambiguous.push(first);
                ambiguous.push(last);
            }
        }

        if ambiguous.is_empty() {
            break;
        }
        ambiguous.sort_unstable();
        ambiguous.dedup();
        for i in ambiguous {
            // Never weaken a proven contradiction to Unknown.
            if result[i].time == FieldConsistency::Consistent {
                result[i].time = FieldConsistency::Unknown;
            }
        }
    }
}

fn time_consistent_indices(result: &[ObservationValidation]) -> Vec<usize> {
    result
        .iter()
        .enumerate()
        .filter_map(|(i, validation)| {
            (validation.time == FieldConsistency::Consistent).then_some(i)
        })
        .collect()
}

fn validate_spatial_consistency(
    observations: &[GpsObservation],
    result: &mut [ObservationValidation],
) {
    // A spatial field is rejected only when two independent kinematic witnesses
    // from the neighbouring observations both prefer the direct bridge over the
    // route through the candidate: reported speed and reported heading. No
    // geographic bounding box, fixed distance, assumed vehicle speed, or time
    // grace period is used.
    //
    // Iterate so that once an isolated damaged position is removed, its clean
    // neighbours can become adjacent and expose another isolated contradiction.
    loop {
        let active = spatially_testable_indices(result);
        if active.len() < 3 {
            break;
        }

        let mut changed = false;
        for window in active.windows(3) {
            let prev = window[0];
            let current = window[1];
            let next = window[2];

            let Some(component_outliers) = spatial_component_outliers(
                &observations[prev],
                &observations[current],
                &observations[next],
            ) else {
                continue;
            };
            if !component_outliers.latitude && !component_outliers.longitude {
                continue;
            }

            let speed_witness = speed_prefers_bridge(
                &observations[prev],
                &observations[current],
                &observations[next],
                result[prev],
                result[next],
            );
            let heading_witness = heading_prefers_bridge(
                &observations[prev],
                &observations[current],
                &observations[next],
                result[prev],
                result[next],
            );

            match combine_independent_witnesses(speed_witness, heading_witness) {
                FieldConsistency::Contradictory => {
                    if component_outliers.latitude
                        && result[current].latitude == FieldConsistency::Consistent
                    {
                        result[current].latitude = FieldConsistency::Contradictory;
                        changed = true;
                    }
                    if component_outliers.longitude
                        && result[current].longitude == FieldConsistency::Consistent
                    {
                        result[current].longitude = FieldConsistency::Contradictory;
                        changed = true;
                    }
                }
                FieldConsistency::Unknown => {
                    if component_outliers.latitude
                        && result[current].latitude == FieldConsistency::Consistent
                    {
                        result[current].latitude = FieldConsistency::Unknown;
                        changed = true;
                    }
                    if component_outliers.longitude
                        && result[current].longitude == FieldConsistency::Consistent
                    {
                        result[current].longitude = FieldConsistency::Unknown;
                        changed = true;
                    }
                }
                FieldConsistency::Consistent => {}
            }
        }

        if !changed {
            break;
        }
    }
}

fn spatially_testable_indices(result: &[ObservationValidation]) -> Vec<usize> {
    result
        .iter()
        .enumerate()
        .filter_map(|(i, validation)| {
            (validation.time == FieldConsistency::Consistent
                && validation.latitude == FieldConsistency::Consistent
                && validation.longitude == FieldConsistency::Consistent)
                .then_some(i)
        })
        .collect()
}

#[derive(Debug, Clone, Copy)]
struct ComponentOutliers {
    latitude: bool,
    longitude: bool,
}

fn spatial_component_outliers(
    prev: &GpsObservation,
    current: &GpsObservation,
    next: &GpsObservation,
) -> Option<ComponentOutliers> {
    let prev_lat = prev.latitude?;
    let current_lat = current.latitude?;
    let next_lat = next.latitude?;
    let prev_lon = prev.longitude?;
    let current_lon = current.longitude?;
    let next_lon = next.longitude?;

    let latitude = outside_closed_interval(current_lat, prev_lat, next_lat);

    // Longitude wraps at +/-180 degrees. Compare all three values on one local
    // unwrapped axis so a legitimate dateline crossing is not an outlier.
    let current_lon_unwrapped = unwrap_longitude_near(prev_lon, current_lon);
    let next_lon_unwrapped = unwrap_longitude_near(prev_lon, next_lon);
    let longitude = outside_closed_interval(current_lon_unwrapped, prev_lon, next_lon_unwrapped);

    Some(ComponentOutliers {
        latitude,
        longitude,
    })
}

fn outside_closed_interval(value: f64, a: f64, b: f64) -> bool {
    value < a.min(b) || value > a.max(b)
}

fn unwrap_longitude_near(reference: f64, longitude: f64) -> f64 {
    let mut delta = (longitude - reference).rem_euclid(360.0);
    if delta > 180.0 {
        delta -= 360.0;
    }
    reference + delta
}

fn combine_independent_witnesses(speed: Option<bool>, heading: Option<bool>) -> FieldConsistency {
    match (speed, heading) {
        (Some(true), Some(true)) => FieldConsistency::Contradictory,
        (Some(false), Some(false)) => FieldConsistency::Consistent,
        (Some(true), None)
        | (None, Some(true))
        | (Some(true), Some(false))
        | (Some(false), Some(true)) => FieldConsistency::Unknown,
        _ => FieldConsistency::Consistent,
    }
}

fn speed_prefers_bridge(
    prev: &GpsObservation,
    current: &GpsObservation,
    next: &GpsObservation,
    prev_validation: ObservationValidation,
    next_validation: ObservationValidation,
) -> Option<bool> {
    if prev_validation.speed != FieldConsistency::Consistent
        || next_validation.speed != FieldConsistency::Consistent
    {
        return None;
    }

    let prev_time = prev.time_ms?;
    let current_time = current.time_ms?;
    let next_time = next.time_ms?;
    let dt_in = (current_time - prev_time) as f64 / 1000.0;
    let dt_out = (next_time - current_time) as f64 / 1000.0;
    let dt_bridge = (next_time - prev_time) as f64 / 1000.0;
    if dt_in <= 0.0 || dt_out <= 0.0 || dt_bridge <= 0.0 {
        return None;
    }

    let prev_pos = (prev.latitude?, prev.longitude?);
    let current_pos = (current.latitude?, current.longitude?);
    let next_pos = (next.latitude?, next.longitude?);
    let via_in_speed = haversine_m(prev_pos, current_pos) / dt_in;
    let via_out_speed = haversine_m(current_pos, next_pos) / dt_out;
    let bridge_speed = haversine_m(prev_pos, next_pos) / dt_bridge;
    let prev_speed = prev.speed_mps?;
    let next_speed = next.speed_mps?;

    Some(bridge_pareto_dominates(
        (bridge_speed - prev_speed).abs(),
        (via_in_speed - prev_speed).abs(),
        (bridge_speed - next_speed).abs(),
        (via_out_speed - next_speed).abs(),
    ))
}

fn heading_prefers_bridge(
    prev: &GpsObservation,
    current: &GpsObservation,
    next: &GpsObservation,
    prev_validation: ObservationValidation,
    next_validation: ObservationValidation,
) -> Option<bool> {
    if prev_validation.heading != FieldConsistency::Consistent
        || next_validation.heading != FieldConsistency::Consistent
    {
        return None;
    }

    let prev_pos = (prev.latitude?, prev.longitude?);
    let current_pos = (current.latitude?, current.longitude?);
    let next_pos = (next.latitude?, next.longitude?);
    let bridge_heading = initial_bearing_deg(prev_pos, next_pos)?;
    let via_in_heading = initial_bearing_deg(prev_pos, current_pos)?;
    let via_out_heading = initial_bearing_deg(current_pos, next_pos)?;
    let prev_heading = prev.heading_deg?;
    let next_heading = next.heading_deg?;

    Some(bridge_pareto_dominates(
        angular_distance_deg(bridge_heading, prev_heading),
        angular_distance_deg(via_in_heading, prev_heading),
        angular_distance_deg(bridge_heading, next_heading),
        angular_distance_deg(via_out_heading, next_heading),
    ))
}

/// The direct bridge is a better witness when it is no worse at either
/// neighbouring observation and strictly better at least once. Equality at one
/// endpoint therefore does not erase independent evidence from the other one,
/// and no epsilon or arbitrary tolerance is introduced.
fn bridge_pareto_dominates(
    bridge_error_a: f64,
    via_error_a: f64,
    bridge_error_b: f64,
    via_error_b: f64,
) -> bool {
    bridge_error_a <= via_error_a
        && bridge_error_b <= via_error_b
        && (bridge_error_a < via_error_a || bridge_error_b < via_error_b)
}

const EARTH_RADIUS_M: f64 = 6_371_000.0;

fn haversine_m(a: (f64, f64), b: (f64, f64)) -> f64 {
    let lat1 = a.0.to_radians();
    let lat2 = b.0.to_radians();
    let dlat = (b.0 - a.0).to_radians();
    let dlon = shortest_longitude_delta_deg(a.1, b.1).to_radians();
    let sin_dlat = (dlat / 2.0).sin();
    let sin_dlon = (dlon / 2.0).sin();
    let h = sin_dlat * sin_dlat + lat1.cos() * lat2.cos() * sin_dlon * sin_dlon;
    let h = h.clamp(0.0, 1.0);
    let central_angle = 2.0 * h.sqrt().atan2((1.0 - h).sqrt());
    EARTH_RADIUS_M * central_angle
}

fn initial_bearing_deg(a: (f64, f64), b: (f64, f64)) -> Option<f64> {
    if a == b {
        return None;
    }
    let lat1 = a.0.to_radians();
    let lat2 = b.0.to_radians();
    let dlon = shortest_longitude_delta_deg(a.1, b.1).to_radians();
    let y = dlon.sin() * lat2.cos();
    let x = lat1.cos() * lat2.sin() - lat1.sin() * lat2.cos() * dlon.cos();
    let bearing = y.atan2(x).to_degrees().rem_euclid(360.0);
    bearing.is_finite().then_some(bearing)
}

fn shortest_longitude_delta_deg(from: f64, to: f64) -> f64 {
    let mut delta = (to - from).rem_euclid(360.0);
    if delta > 180.0 {
        delta -= 360.0;
    }
    delta
}

fn angular_distance_deg(a: f64, b: f64) -> f64 {
    shortest_longitude_delta_deg(a, b).abs()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eastbound_observations() -> Vec<GpsObservation> {
        // About 11.1 metres east per second at the equator.
        (0..5)
            .map(|i| GpsObservation {
                source_order: i,
                time_ms: Some(i as i64 * 1000),
                latitude: Some(0.0),
                longitude: Some(i as f64 * 0.0001),
                speed_mps: Some(11.12),
                heading_deg: Some(90.0),
            })
            .collect()
    }

    #[test]
    fn exact_zero_zero_is_valid_when_sequence_is_consistent() {
        let observations = vec![
            GpsObservation {
                source_order: 0,
                time_ms: Some(0),
                latitude: Some(0.0),
                longitude: Some(-0.0001),
                speed_mps: Some(11.12),
                heading_deg: Some(90.0),
            },
            GpsObservation {
                source_order: 1,
                time_ms: Some(1000),
                latitude: Some(0.0),
                longitude: Some(0.0),
                speed_mps: Some(11.12),
                heading_deg: Some(90.0),
            },
            GpsObservation {
                source_order: 2,
                time_ms: Some(2000),
                latitude: Some(0.0),
                longitude: Some(0.0001),
                speed_mps: Some(11.12),
                heading_deg: Some(90.0),
            },
        ];
        let validated = validate(&observations, ValidationContext::default());
        assert!(validated[1].usable_timed_position());
    }

    #[test]
    fn isolated_wrong_timestamp_is_rejected_without_timezone_assumption() {
        let mut observations = eastbound_observations();
        observations[2].time_ms = Some(-10_000_000);
        let validated = validate(&observations, ValidationContext::default());
        assert_eq!(validated[2].time, FieldConsistency::Contradictory);
        assert!(!validated[2].usable_timed_position());
        assert!(validated[1].usable_timed_position());
        assert!(validated[3].usable_timed_position());
    }

    #[test]
    fn one_coordinate_field_can_fail_without_discarding_the_other_field() {
        let mut observations = eastbound_observations();
        observations[2].longitude = Some(20.0);
        let validated = validate(&observations, ValidationContext::default());
        assert_eq!(validated[2].latitude, FieldConsistency::Consistent);
        assert_eq!(validated[2].longitude, FieldConsistency::Contradictory);
        assert!(!validated[2].usable_timed_position());
    }

    #[test]
    fn unresolved_time_conflict_becomes_unknown_instead_of_guessing() {
        let observations = vec![
            GpsObservation {
                source_order: 0,
                time_ms: Some(0),
                latitude: Some(0.0),
                longitude: Some(0.0),
                speed_mps: Some(0.0),
                heading_deg: Some(0.0),
            },
            GpsObservation {
                source_order: 1,
                time_ms: Some(3_600_000),
                latitude: Some(0.0),
                longitude: Some(0.0),
                speed_mps: Some(0.0),
                heading_deg: Some(0.0),
            },
        ];
        let validated = validate(
            &observations,
            ValidationContext {
                max_time_span_ms: Some(60_000),
            },
        );
        assert_eq!(validated[0].time, FieldConsistency::Unknown);
        assert_eq!(validated[1].time, FieldConsistency::Unknown);
    }

    #[test]
    fn all_field_corruption_combinations_are_representable() {
        // Exercise every one of the 2^5 combinations of a damaged middle
        // record. Route usability must depend only on time + complete position;
        // speed/heading corruption must not itself destroy a sound position.
        for mask in 0u8..32 {
            let mut observations = eastbound_observations();
            if mask & 0b00001 != 0 {
                observations[2].time_ms = Some(-10_000_000);
            }
            if mask & 0b00010 != 0 {
                observations[2].latitude = Some(20.0);
            }
            if mask & 0b00100 != 0 {
                observations[2].longitude = Some(20.0);
            }
            if mask & 0b01000 != 0 {
                observations[2].speed_mps = Some(0.0);
            }
            if mask & 0b10000 != 0 {
                observations[2].heading_deg = Some(0.0);
            }

            let validated = validate(&observations, ValidationContext::default());
            let route_fields_corrupt = mask & 0b00111 != 0;
            assert_eq!(
                validated[2].usable_timed_position(),
                !route_fields_corrupt,
                "unexpected route usability for mask {mask:05b}"
            );
        }
    }

    #[test]
    fn intrinsic_invalid_values_are_field_local() {
        let mut observations = eastbound_observations();
        observations[2].speed_mps = Some(-1.0);
        observations[2].heading_deg = Some(f64::NAN);
        let validated = validate(&observations, ValidationContext::default());
        assert_eq!(validated[2].speed, FieldConsistency::Contradictory);
        assert_eq!(validated[2].heading, FieldConsistency::Contradictory);
        assert!(validated[2].usable_timed_position());
    }
}
