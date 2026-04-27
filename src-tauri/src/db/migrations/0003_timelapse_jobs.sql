CREATE TABLE timelapse_jobs (
    trip_id TEXT NOT NULL,
    tier TEXT NOT NULL,
    channel TEXT NOT NULL,
    status TEXT NOT NULL,
    output_path TEXT,
    error_message TEXT,
    ffmpeg_version TEXT,
    encoder_used TEXT,
    created_at_ms INTEGER NOT NULL,
    completed_at_ms INTEGER,
    PRIMARY KEY (trip_id, tier, channel)
);
CREATE INDEX idx_timelapse_jobs_status ON timelapse_jobs(status);
CREATE INDEX idx_timelapse_jobs_trip ON timelapse_jobs(trip_id);
