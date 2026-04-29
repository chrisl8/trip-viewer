-- Manual trip-merge directives. Each row says "the natural trip with id
-- = absorbed_trip_id should be folded into the trip with id =
-- primary_trip_id whenever grouping runs." Survives a folder rescan
-- because natural trip IDs are deterministic (derived from the first
-- segment's hash-stable ID) — as long as the same files are present in
-- the same order, the same absorbed_trip_id will reappear and the merge
-- will reapply.
--
-- Keyed on absorbed_trip_id because each natural trip can only belong to
-- one merged trip. primary_trip_id may itself appear as a natural trip
-- in `trips` (it's just the survivor) or, less commonly, may not (the
-- caller may have generated a fresh primary).
--
-- No FK to `trips`. The whole point of this table is to outlive trip-row
-- rewrites: persist_and_gc deletes the absorbed trip's row, and we want
-- the merge directive to remain so the next rescan reapplies it.

CREATE TABLE manual_trip_merges (
    absorbed_trip_id TEXT PRIMARY KEY,
    primary_trip_id  TEXT NOT NULL,
    created_ms       INTEGER NOT NULL
);
CREATE INDEX idx_manual_trip_merges_primary ON manual_trip_merges(primary_trip_id);
