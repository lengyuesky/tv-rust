CREATE TABLE fntv_record_links (
 auth_hash TEXT NOT NULL,
 link_hash TEXT NOT NULL,
 menu_id TEXT NOT NULL REFERENCES fntv_playback_menus(id) ON DELETE CASCADE,
 play_id TEXT NOT NULL REFERENCES plays(id) ON DELETE CASCADE,
 PRIMARY KEY(auth_hash,link_hash)
);
ALTER TABLE media_assets ADD COLUMN crypto TEXT NOT NULL DEFAULT '';
