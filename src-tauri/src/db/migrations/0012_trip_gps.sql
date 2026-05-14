-- Trip-level archived GPS. Populated as a side effect of the timelapse
-- encoder's per-trip stitch step (see timelapse/worker.rs::build_trip_context),
-- so any trip with a completed timelapse can render its map + speed graph
-- after the originals are deleted.
--
-- Trip-level (not segment-level) because segments may be hard-deleted on
-- the path to archive-only (migration 0010); only the trip row survives
-- the transition. JSON blob matches the existing speed_curve_json pattern
-- (migration 0006) -- the access pattern is load-whole, never range-query.
--
-- Already-archive-only trips with their originals already trashed are
-- unrecoverable: their GPS went with the MP4 files. This table only
-- protects trips going forward and trips that still have originals on
-- disk (caught by the backfill pass in timelapse/cleanup.rs).
CREATE TABLE trip_gps (
    trip_id        TEXT PRIMARY KEY REFERENCES trips(id) ON DELETE CASCADE,
    points_json    TEXT    NOT NULL,
    point_count    INTEGER NOT NULL,
    parser_version INTEGER NOT NULL,
    created_at_ms  INTEGER NOT NULL
);
