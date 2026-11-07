-- NULL marks rows written without source/health provenance (including legacy
-- timelapse writers). Keep their GPS, but validate originals before reuse.
ALTER TABLE trip_gps ADD COLUMN source_signature TEXT;
