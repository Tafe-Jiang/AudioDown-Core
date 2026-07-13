CREATE TABLE credentials (
  id TEXT PRIMARY KEY,
  kind TEXT NOT NULL CHECK (kind IN ('cookie', 'token')),
  platform_id TEXT NOT NULL,
  scope TEXT NOT NULL UNIQUE,
  source_plugin_id TEXT,
  algorithm_version INTEGER NOT NULL
    CHECK (algorithm_version BETWEEN 1 AND 65535),
  key_version INTEGER NOT NULL
    CHECK (key_version BETWEEN 1 AND 4294967295),
  nonce BLOB NOT NULL CHECK (length(nonce) = 12),
  ciphertext BLOB NOT NULL
    CHECK (length(ciphertext) BETWEEN 16 AND 65552),
  status TEXT NOT NULL
    CHECK (status IN ('active', 'expired', 'revoked', 'error')),
  account_id_hint TEXT
    CHECK (account_id_hint IS NULL OR length(account_id_hint) BETWEEN 1 AND 256),
  display_name TEXT
    CHECK (display_name IS NULL OR length(display_name) BETWEEN 1 AND 256),
  safe_error_summary TEXT
    CHECK (safe_error_summary IS NULL OR length(safe_error_summary) BETWEEN 1 AND 512),
  expires_at TEXT,
  status_checked_at TEXT,
  record_revision INTEGER NOT NULL DEFAULT 1 CHECK (record_revision > 0),
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  FOREIGN KEY (source_plugin_id) REFERENCES plugins(plugin_id) ON DELETE RESTRICT
);

CREATE TABLE credential_target_origins (
  credential_id TEXT NOT NULL,
  origin TEXT NOT NULL,
  PRIMARY KEY (credential_id, origin),
  FOREIGN KEY (credential_id) REFERENCES credentials(id) ON DELETE CASCADE
);

CREATE TABLE credential_scope_grants (
  id TEXT PRIMARY KEY,
  plugin_id TEXT NOT NULL,
  manifest_hash TEXT NOT NULL,
  credential_id TEXT NOT NULL,
  scope TEXT NOT NULL,
  credential_origins_hash BLOB NOT NULL
    CHECK (length(credential_origins_hash) = 32),
  created_at TEXT NOT NULL,
  revoked_at TEXT,
  UNIQUE (id, credential_id),
  FOREIGN KEY (plugin_id) REFERENCES plugins(plugin_id) ON DELETE CASCADE,
  FOREIGN KEY (credential_id) REFERENCES credentials(id) ON DELETE CASCADE
);

CREATE TABLE credential_scope_grant_origins (
  grant_id TEXT NOT NULL,
  credential_id TEXT NOT NULL,
  origin TEXT NOT NULL,
  PRIMARY KEY (grant_id, origin),
  FOREIGN KEY (grant_id, credential_id)
    REFERENCES credential_scope_grants(id, credential_id)
    ON DELETE CASCADE
);

CREATE INDEX idx_credentials_platform_status
  ON credentials(platform_id, status, scope);

CREATE INDEX idx_credentials_source_plugin
  ON credentials(source_plugin_id, scope);

CREATE UNIQUE INDEX idx_active_credential_scope_grants
  ON credential_scope_grants(plugin_id, credential_id, scope)
  WHERE revoked_at IS NULL;

CREATE INDEX idx_credential_scope_grants_plugin
  ON credential_scope_grants(plugin_id, revoked_at, created_at);

CREATE INDEX idx_credential_scope_grants_credential
  ON credential_scope_grants(credential_id, scope, revoked_at);
