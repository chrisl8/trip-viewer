-- Normalize `timelapse_jobs.output_path` from absolute to archive-relative
-- without losing information about the on-disk filename.
--
-- Older code stored the absolute path produced by `output_root.join(...)`,
-- which broke whenever the archive drive remounted at a different path.
--
-- Critically: we must NOT re-derive the filename from the row's current
-- `trip_id`, `tier`, `channel` columns. If `rebuild_for_cross_os`
-- previously updated the trip_id column without renaming the on-disk
-- file in lockstep (which is exactly what it does), the row's current
-- trip_id no longer matches the trip_id portion baked into the filename.
-- Re-deriving from columns would silently overwrite the only remaining
-- piece of state that records the actual on-disk filename — and the
-- mapping cannot be reconstructed afterward.
--
-- Instead: keep the existing filename basename intact and only rewrite
-- the directory portion to the canonical `Timelapses/` relative form.
-- The encoder's naming guarantee is `{trip_id}_{tier}_{channel}.mp4`,
-- where trip_id is a 36-char UUID and the suffix `_<tier>_<channel>.mp4`
-- has length `1 + length(tier) + 1 + length(channel) + 4` =
-- `length(tier) + length(channel) + 6`. So the basename has fixed length
-- `36 + length(tier) + length(channel) + 6` and we extract it from the
-- end of `output_path` via a negative-offset `substr` (safe even if the
-- column happens to be shorter — SQLite truncates to the whole string).
--
-- If a future filename shape diverges, this still does the right thing:
-- it preserves whatever the column held, just stripped of any directory
-- prefix it had.
UPDATE timelapse_jobs
SET output_path = 'Timelapses/' || substr(
    output_path,
    -(36 + length(tier) + length(channel) + 6)
)
WHERE output_path IS NOT NULL;
