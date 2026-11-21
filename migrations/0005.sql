CREATE TABLE fntv_works (
 id TEXT PRIMARY KEY,
 group_key TEXT NOT NULL UNIQUE,
 season_id TEXT NOT NULL UNIQUE
);
CREATE TABLE fntv_work_episodes (
 id TEXT PRIMARY KEY,
 work_id TEXT NOT NULL REFERENCES fntv_works(id),
 episode_number INTEGER NOT NULL,
 UNIQUE(work_id,episode_number)
);
CREATE TABLE fntv_playback_menus (
 id TEXT PRIMARY KEY,
 access_hash TEXT NOT NULL UNIQUE,
 user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
 auth_hash TEXT NOT NULL,
 reference_episode_id TEXT NOT NULL REFERENCES episodes(id),
 choices TEXT NOT NULL,
 origin TEXT NOT NULL,
 active_play_id TEXT REFERENCES plays(id),
 expires_at INTEGER NOT NULL,
 recording_disabled INTEGER NOT NULL DEFAULT 0 CHECK(recording_disabled IN (0,1)),
 closed INTEGER NOT NULL DEFAULT 0 CHECK(closed IN (0,1))
);
CREATE INDEX fntv_menus_auth_episode ON fntv_playback_menus(auth_hash,reference_episode_id,expires_at);
