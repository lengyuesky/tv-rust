ALTER TABLE subscriptions ADD COLUMN all_users INTEGER NOT NULL DEFAULT 1 CHECK(all_users IN (0,1));
CREATE TABLE subscription_users (
 subscription_id TEXT NOT NULL REFERENCES subscriptions(id) ON DELETE CASCADE,
 user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
 PRIMARY KEY(subscription_id,user_id)
);
CREATE INDEX subscription_users_by_user ON subscription_users(user_id,subscription_id);
