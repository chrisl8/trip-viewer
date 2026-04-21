CREATE TABLE segments (
    id TEXT PRIMARY KEY,
    trip_id TEXT NOT NULL,
    start_time_ms INTEGER NOT NULL,
    duration_s REAL NOT NULL,
    master_path TEXT NOT NULL,
    is_event INTEGER NOT NULL DEFAULT 0,
    camera_kind TEXT NOT NULL DEFAULT 'generic',
    gps_supported INTEGER NOT NULL DEFAULT 0,
    last_seen_ms INTEGER NOT NULL
);
CREATE INDEX idx_segments_trip ON segments(trip_id);

CREATE TABLE trips (
    id TEXT PRIMARY KEY,
    start_time_ms INTEGER NOT NULL,
    end_time_ms INTEGER NOT NULL,
    last_seen_ms INTEGER NOT NULL
);

CREATE TABLE tags (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    segment_id TEXT,
    trip_id TEXT,
    name TEXT NOT NULL,
    category TEXT NOT NULL,
    source TEXT NOT NULL,
    scan_id TEXT,
    scan_version INTEGER,
    confidence REAL,
    start_ms INTEGER,
    end_ms INTEGER,
    note TEXT,
    metadata_json TEXT,
    created_ms INTEGER NOT NULL,
    CHECK ((segment_id IS NOT NULL) OR (trip_id IS NOT NULL))
);
CREATE INDEX idx_tags_segment ON tags(segment_id);
CREATE INDEX idx_tags_trip ON tags(trip_id);
CREATE INDEX idx_tags_name ON tags(name);
CREATE INDEX idx_tags_scan ON tags(scan_id);

CREATE TABLE scan_runs (
    segment_id TEXT NOT NULL,
    scan_id TEXT NOT NULL,
    version INTEGER NOT NULL,
    ran_at_ms INTEGER NOT NULL,
    status TEXT NOT NULL,
    error_message TEXT,
    PRIMARY KEY (segment_id, scan_id)
);
