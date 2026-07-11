ALTER TABLE plugins
  ADD COLUMN search_enabled INTEGER NOT NULL DEFAULT 1
  CHECK (search_enabled IN (0, 1));

ALTER TABLE plugins
  ADD COLUMN discover_enabled INTEGER NOT NULL DEFAULT 1
  CHECK (discover_enabled IN (0, 1));

CREATE TABLE platform_content_defaults (
  platform_id TEXT PRIMARY KEY,
  plugin_id TEXT NOT NULL UNIQUE,
  updated_at TEXT NOT NULL,
  FOREIGN KEY (plugin_id) REFERENCES plugins(plugin_id) ON DELETE CASCADE
);

CREATE INDEX idx_plugins_content_routing
  ON plugins(plugin_type, enabled, platform_id, priority, plugin_id);
