-- Tombstone segments: rows that record where an original segment used to
-- live, after the user deleted its source files but the trip's timelapse
-- archive remains. The row preserves `start_time_ms` and `duration_s` so
-- the timeline math stays correct in the mixed state (some originals
-- survive, some are gone). `master_path` is set to '' on tombstone rows.
--
-- Tombstones are only created when a completed timelapse exists for the
-- trip — otherwise there is nothing to play across the deleted span and
-- the segment is hard-deleted as before. When the last surviving original
-- in a trip is tombstoned, all that trip's tombstones are hard-deleted so
-- the trip flips cleanly to archive-only via the existing path.
--
-- GC rule update: `last_seen_ms` is meaningless for tombstones (no file to
-- "see"), so the persist_and_gc DELETE is amended in code to exclude
-- `is_tombstone = 1` rows. They live until either the trip is fully
-- deleted or the user re-imports footage that re-binds the row (an edge
-- case the upsert path handles by clearing the flag).

ALTER TABLE segments ADD COLUMN is_tombstone INTEGER NOT NULL DEFAULT 0;
CREATE INDEX idx_segments_trip_tombstone ON segments(trip_id, is_tombstone);
