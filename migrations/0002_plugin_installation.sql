ALTER TABLE plugins ADD COLUMN repository_id TEXT;
ALTER TABLE plugins ADD COLUMN source_hash TEXT;
ALTER TABLE plugins ADD COLUMN last_used_at TEXT;
ALTER TABLE plugins ADD COLUMN install_operation_id TEXT;

CREATE TABLE plugin_risk_grants (
  id TEXT PRIMARY KEY,
  repository_id TEXT NOT NULL,
  plugin_id TEXT NOT NULL,
  commit_sha TEXT NOT NULL,
  risk_kind TEXT NOT NULL,
  reason TEXT NOT NULL,
  granted_at TEXT NOT NULL,
  UNIQUE(plugin_id, commit_sha, risk_kind)
);

CREATE INDEX idx_plugin_risk_grants_plugin
  ON plugin_risk_grants(plugin_id, commit_sha);
