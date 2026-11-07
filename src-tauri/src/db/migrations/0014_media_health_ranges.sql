-- Per-file decode failure intervals discovered during playback.
--
-- Rows are tied to a file fingerprint (size + modified timestamp). If a source
-- file is repaired or replaced in-place, old intervals are ignored because the
-- fingerprint no longer matches.
--
-- end_ms NULL means "failure observed from start_ms, recovery not yet seen".
-- Closed ranges let the player skip known-bad spans without repeatedly asking
-- the decoder to rediscover the same corruption on every viewing.
CREATE TABLE media_health_ranges (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    file_path     TEXT    NOT NULL,
    file_size     INTEGER NOT NULL,
    modified_ms   INTEGER NOT NULL,
    start_ms      INTEGER NOT NULL,
    end_ms        INTEGER,
    error_kind    TEXT    NOT NULL,
    updated_at_ms INTEGER NOT NULL
);

CREATE INDEX idx_media_health_lookup
    ON media_health_ranges(file_path, file_size, modified_ms, start_ms);
