-- Trip-level metadata so a trip with no current segments (archive-only:
-- sources deleted, timelapse remains) can still be played back without
-- needing to read fields from a non-existent segment row. Mirrors the
-- segments-table columns. Default 'generic' / 0 are placeholders for
-- migrated rows that have no segments to backfill from; new trips
-- always overwrite these via upsert_trip from segments[0].
ALTER TABLE trips ADD COLUMN camera_kind TEXT NOT NULL DEFAULT 'generic';
ALTER TABLE trips ADD COLUMN gps_supported INTEGER NOT NULL DEFAULT 0;

-- Backfill from any current segment of each trip. A trip with no segments
-- (already archive-only at migration time, theoretically impossible until
-- this PR ships but defensive) keeps the defaults.
UPDATE trips
SET
    camera_kind = COALESCE(
        (SELECT camera_kind FROM segments WHERE segments.trip_id = trips.id LIMIT 1),
        camera_kind
    ),
    gps_supported = COALESCE(
        (SELECT gps_supported FROM segments WHERE segments.trip_id = trips.id LIMIT 1),
        gps_supported
    );
