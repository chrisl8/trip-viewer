CREATE TABLE trip_bookmarks (
    trip_id TEXT NOT NULL REFERENCES trips(id) ON DELETE CASCADE ON UPDATE CASCADE,
    time_ms INTEGER NOT NULL CHECK (time_ms >= 0),
    PRIMARY KEY (trip_id, time_ms)
);
