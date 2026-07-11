PRAGMA foreign_keys = ON;

CREATE TABLE system_meta (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE TABLE plugins (
  plugin_id TEXT PRIMARY KEY,
  plugin_type TEXT NOT NULL,
  platform_id TEXT NOT NULL,
  name TEXT NOT NULL,
  version TEXT NOT NULL,
  protocol_version TEXT NOT NULL,
  source_kind TEXT NOT NULL,
  source_ref TEXT NOT NULL,
  commit_sha TEXT,
  manifest_json TEXT NOT NULL,
  manifest_hash TEXT NOT NULL,
  image_id TEXT,
  status TEXT NOT NULL,
  run_mode TEXT NOT NULL DEFAULT 'on_demand',
  priority INTEGER NOT NULL DEFAULT 100,
  enabled INTEGER NOT NULL DEFAULT 1,
  last_error TEXT,
  installed_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE TABLE structured_logs (
  id TEXT PRIMARY KEY,
  timestamp TEXT NOT NULL,
  level TEXT NOT NULL,
  component TEXT NOT NULL,
  message TEXT NOT NULL,
  plugin_id TEXT,
  plugin_version TEXT,
  platform_id TEXT,
  request_id TEXT,
  task_id TEXT,
  container_id TEXT,
  error_code TEXT,
  context_json TEXT NOT NULL DEFAULT '{}'
);

CREATE INDEX idx_structured_logs_timestamp ON structured_logs(timestamp DESC);
CREATE INDEX idx_structured_logs_plugin ON structured_logs(plugin_id, timestamp DESC);
CREATE INDEX idx_structured_logs_request ON structured_logs(request_id, timestamp DESC);
