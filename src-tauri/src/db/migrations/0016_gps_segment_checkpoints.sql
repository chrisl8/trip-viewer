-- Raw segment GPS, before cross-file boundary validation. Separate from the
-- complete archived trip track: an interrupted analysis must not mark a partial
-- trip as complete. Provenance includes source fingerprints and media health.
CREATE TABLE gps_segment_checkpoints (
    trip_id TEXT NOT NULL REFERENCES trips(id) ON DELETE CASCADE,
    segment_id TEXT NOT NULL,
    source_signature TEXT NOT NULL,
    parser_version INTEGER NOT NULL,
    payload_json TEXT NOT NULL,
    PRIMARY KEY (trip_id, segment_id)
);
