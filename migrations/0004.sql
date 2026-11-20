ALTER TABLE plays ADD COLUMN recording_disabled INTEGER NOT NULL DEFAULT 0 CHECK(recording_disabled IN (0,1));
CREATE INDEX plays_user_film ON plays(user_id,film_id,seq);
CREATE TABLE fntv_source_versions (
 id TEXT PRIMARY KEY,
 film_id TEXT NOT NULL REFERENCES films(id),
 line_name TEXT NOT NULL,
 episode_id TEXT NOT NULL REFERENCES episodes(id)
);
