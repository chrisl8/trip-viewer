-- Per-trip storage visibility. Two nullable columns added in one
-- migration so the UI can show users where their disk is going and
-- how much they'd reclaim by deleting originals on already-timelapsed
-- trips.
--
-- NULL is the "unknown" sentinel, distinct from 0:
--   * `segments.size_bytes` is NULL for any pre-migration row until
--     the next folder scan stamps it. The scan path already touches
--     every live segment, so no separate backfill is needed.
--   * `timelapse_jobs.output_size_bytes` is NULL for any job that
--     completed before this migration. `cleanup_stale_jobs` runs a
--     one-shot pass on startup that stat's each `output_path` and
--     fills the column.
--
-- Archive-only trips (originals already deleted) keep `size_bytes`
-- as NULL forever — the rows are already GC'd. The frontend
-- `formatBytes(null)` renders these as "—".

ALTER TABLE segments ADD COLUMN size_bytes INTEGER;
ALTER TABLE timelapse_jobs ADD COLUMN output_size_bytes INTEGER;
