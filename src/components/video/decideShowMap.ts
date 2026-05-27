/**
 * Decide whether the MapPanel should render in PlayerShell.
 *
 * Extracted as a pure function so the gating rule can be regression-
 * tested without spinning up React. The rule is non-obvious: a stale
 * version of this gate hid the map for ALL archive-only trips, but
 * since the `trip_gps` table was introduced (commit de1acb4) the map
 * has stitched GPS available for archive-only trips that were
 * timelapsed after that feature shipped. Pre-feature archive-only
 * trips genuinely have no GPS source and the map should stay
 * collapsed for them.
 *
 * Rules, in order of precedence:
 *   1. If the camera doesn't record GPS, never show the map.
 *   2. If the trip is live (has segments), show the map. Per-segment
 *      GPS extraction handles the data path.
 *   3. If the trip is archive-only AND has persisted GPS in
 *      `trip_gps` (point count > 0), show the map.
 *   4. Otherwise (archive-only with no persisted GPS), collapse it.
 */
export interface ShowMapInputs {
  /** True when the active segment / trip records GPS data. */
  gpsSupported: boolean;
  /** True when the trip has zero live segments. */
  archiveOnly: boolean;
  /** Number of GPS points in the persisted trip_gps row (0 if no row). */
  archivedGpsPointCount: number;
}

export function decideShowMap(inputs: ShowMapInputs): boolean {
  if (!inputs.gpsSupported) return false;
  if (!inputs.archiveOnly) return true;
  return inputs.archivedGpsPointCount > 0;
}
