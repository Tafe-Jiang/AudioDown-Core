use std::{collections::HashSet, fmt};

use audiodown_domain::{
    credential::{
        CredentialId, CredentialKind, CredentialOwnership, CredentialPublicMetadata,
        CredentialScope, CredentialStatus,
    },
    plugin::PluginId,
};
use audiodown_plugin_api::manifest::{
    CredentialDeclarations, CredentialTargetOrigin, MAX_CREDENTIAL_TARGET_ORIGINS,
};
use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use sqlx::{Row, Sqlite, SqlitePool, Transaction};
use uuid::Uuid;

use crate::StorageError;

const NONCE_BYTES: usize = 12;
const MIN_CIPHERTEXT_BYTES: usize = 16;
const MAX_CIPHERTEXT_BYTES: usize = 65_552;
const MAX_SAFE_METADATA_BYTES: usize = 512;
const ORIGIN_HASH_BYTES: usize = 32;

#[derive(Clone, PartialEq, Eq)]
pub struct CredentialRecord {
    pub id: CredentialId,
    pub kind: CredentialKind,
    pub platform_id: String,
    pub scope: CredentialScope,
    pub source_plugin_id: Option<PluginId>,
    pub algorithm_version: u16,
    pub key_version: u32,
    pub nonce: [u8; NONCE_BYTES],
    pub ciphertext: Vec<u8>,
    pub target_origins: Vec<CredentialTargetOrigin>,
    pub status: CredentialStatus,
    pub account_id_hint: Option<String>,
    pub display_name: Option<String>,
    pub safe_error_summary: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub status_checked_at: Option<DateTime<Utc>>,
    pub revision: u64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl CredentialRecord {
    pub fn public_metadata(&self) -> CredentialPublicMetadata {
        CredentialPublicMetadata {
            id: self.id,
            kind: self.kind,
            platform_id: self.platform_id.clone(),
            scope: self.scope.clone(),
            ownership: match &self.source_plugin_id {
                Some(plugin_id) => CredentialOwnership::Plugin(plugin_id.clone()),
                None => CredentialOwnership::Retained,
            },
            status: self.status,
            expires_at: self.expires_at,
            created_at: self.created_at,
            updated_at: self.updated_at,
        }
    }
}

impl fmt::Debug for CredentialRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CredentialRecord")
            .field("id", &self.id)
            .field("kind", &self.kind)
            .field("platform_id", &self.platform_id)
            .field("scope", &self.scope)
            .field("source_plugin_id", &self.source_plugin_id)
            .field("algorithm_version", &self.algorithm_version)
            .field("key_version", &self.key_version)
            .field("nonce", &"[REDACTED]")
            .field(
                "ciphertext",
                &format!("[REDACTED; {} bytes]", self.ciphertext.len()),
            )
            .field("target_origins", &self.target_origins)
            .field("status", &self.status)
            .field("account_id_hint", &self.account_id_hint)
            .field("display_name", &self.display_name)
            .field("safe_error_summary", &self.safe_error_summary)
            .field("expires_at", &self.expires_at)
            .field("status_checked_at", &self.status_checked_at)
            .field("revision", &self.revision)
            .field("created_at", &self.created_at)
            .field("updated_at", &self.updated_at)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialScopeGrantRecord {
    pub id: Uuid,
    pub plugin_id: PluginId,
    pub manifest_hash: String,
    pub credential_id: CredentialId,
    pub scope: CredentialScope,
    pub target_origins: Vec<CredentialTargetOrigin>,
    pub created_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
}

pub struct CredentialRepository<'a> {
    pool: &'a SqlitePool,
}

impl<'a> CredentialRepository<'a> {
    pub(crate) fn new(pool: &'a SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn insert(&self, record: &CredentialRecord) -> Result<(), StorageError> {
        validate_credential_record(record)?;
        if record.revision != 1 {
            return Err(invalid_data("new credential revision must be one"));
        }

        let origins = normalize_origins(&record.target_origins)?;
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        validate_source_plugin(&mut transaction, record).await?;
        insert_credential(&mut transaction, record).await?;
        replace_credential_origins(&mut transaction, record.id, &origins).await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn upsert(&self, record: &CredentialRecord) -> Result<u64, StorageError> {
        validate_credential_record(record)?;
        let origins = normalize_origins(&record.target_origins)?;
        let next_revision = record
            .revision
            .checked_add(1)
            .ok_or_else(|| invalid_data("credential revision is invalid"))?;
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        validate_source_plugin(&mut transaction, record).await?;

        let result = sqlx::query(
            r#"
            UPDATE credentials
            SET kind = ?, platform_id = ?, scope = ?, source_plugin_id = ?,
                algorithm_version = ?, key_version = ?, nonce = ?, ciphertext = ?,
                status = ?, account_id_hint = ?, display_name = ?,
                safe_error_summary = ?, expires_at = ?, status_checked_at = ?,
                record_revision = ?, updated_at = ?
            WHERE id = ? AND record_revision = ?
            "#,
        )
        .bind(credential_kind_to_str(record.kind))
        .bind(&record.platform_id)
        .bind(record.scope.as_str())
        .bind(record.source_plugin_id.as_ref().map(PluginId::as_str))
        .bind(i64::from(record.algorithm_version))
        .bind(i64::from(record.key_version))
        .bind(record.nonce.as_slice())
        .bind(&record.ciphertext)
        .bind(credential_status_to_str(record.status))
        .bind(&record.account_id_hint)
        .bind(&record.display_name)
        .bind(&record.safe_error_summary)
        .bind(record.expires_at.map(|timestamp| timestamp.to_rfc3339()))
        .bind(
            record
                .status_checked_at
                .map(|timestamp| timestamp.to_rfc3339()),
        )
        .bind(u64_to_i64(next_revision)?)
        .bind(record.updated_at.to_rfc3339())
        .bind(record.id.to_string())
        .bind(u64_to_i64(record.revision)?)
        .execute(&mut *transaction)
        .await
        .map_err(map_write_error)?;

        if result.rows_affected() == 1 {
            replace_credential_origins(&mut transaction, record.id, &origins).await?;
            transaction.commit().await?;
            return Ok(next_revision);
        }

        let exists: i64 =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM credentials WHERE id = ?)")
                .bind(record.id.to_string())
                .fetch_one(&mut *transaction)
                .await?;
        if exists != 0 {
            return Err(StorageError::Conflict);
        }
        if record.revision != 1 {
            return Err(StorageError::Conflict);
        }

        insert_credential(&mut transaction, record).await?;
        replace_credential_origins(&mut transaction, record.id, &origins).await?;
        transaction.commit().await?;
        Ok(1)
    }

    pub async fn get(
        &self,
        credential_id: &CredentialId,
    ) -> Result<Option<CredentialRecord>, StorageError> {
        let mut transaction = self.pool.begin().await?;
        let row = sqlx::query("SELECT * FROM credentials WHERE id = ?")
            .bind(credential_id.to_string())
            .fetch_optional(&mut *transaction)
            .await?;
        let record = match row {
            Some(row) => {
                let mut record = decode_credential(row)?;
                record.target_origins =
                    load_credential_origins_from_transaction(&mut transaction, record.id).await?;
                Some(record)
            }
            None => None,
        };
        transaction.commit().await?;
        Ok(record)
    }

    pub async fn get_by_scope(
        &self,
        scope: &CredentialScope,
    ) -> Result<Option<CredentialRecord>, StorageError> {
        let mut transaction = self.pool.begin().await?;
        let row = sqlx::query("SELECT * FROM credentials WHERE scope = ?")
            .bind(scope.as_str())
            .fetch_optional(&mut *transaction)
            .await?;
        let record = match row {
            Some(row) => {
                let mut record = decode_credential(row)?;
                record.target_origins =
                    load_credential_origins_from_transaction(&mut transaction, record.id).await?;
                Some(record)
            }
            None => None,
        };
        transaction.commit().await?;
        Ok(record)
    }

    pub async fn list(&self) -> Result<Vec<CredentialRecord>, StorageError> {
        let mut transaction = self.pool.begin().await?;
        let rows = sqlx::query("SELECT * FROM credentials ORDER BY scope ASC")
            .fetch_all(&mut *transaction)
            .await?;
        let mut records = Vec::with_capacity(rows.len());
        for row in rows {
            let mut record = decode_credential(row)?;
            record.target_origins =
                load_credential_origins_from_transaction(&mut transaction, record.id).await?;
            records.push(record);
        }
        transaction.commit().await?;
        Ok(records)
    }

    pub async fn clear_source_plugin(
        &self,
        credential_id: &CredentialId,
    ) -> Result<(), StorageError> {
        let result = sqlx::query(
            r#"
            UPDATE credentials
            SET source_plugin_id = NULL,
                record_revision = record_revision + 1,
                updated_at = ?
            WHERE id = ?
            "#,
        )
        .bind(Utc::now().to_rfc3339())
        .bind(credential_id.to_string())
        .execute(self.pool)
        .await?;
        ensure_one_row(result.rows_affected())
    }

    pub async fn delete(&self, credential_id: &CredentialId) -> Result<(), StorageError> {
        let result = sqlx::query("DELETE FROM credentials WHERE id = ?")
            .bind(credential_id.to_string())
            .execute(self.pool)
            .await?;
        ensure_one_row(result.rows_affected())
    }

    pub async fn create_grant(
        &self,
        record: &CredentialScopeGrantRecord,
    ) -> Result<(), StorageError> {
        validate_grant_record(record)?;
        let grant_origins = normalize_origins(&record.target_origins)?;
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;

        let plugin = sqlx::query(
            r#"
            SELECT plugin_type, platform_id, manifest_hash, manifest_json
            FROM plugins
            WHERE plugin_id = ?
            "#,
        )
        .bind(record.plugin_id.as_str())
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(StorageError::NotFound)?;
        let plugin_type: String = plugin.try_get("plugin_type")?;
        let plugin_platform: String = plugin.try_get("platform_id")?;
        let manifest_hash: String = plugin.try_get("manifest_hash")?;
        let manifest_json: String = plugin.try_get("manifest_json")?;
        if plugin_type != "content"
            || manifest_hash != record.manifest_hash
            || plugin_platform.is_empty()
        {
            return Err(invalid_data(
                "credential grant requires the current content plugin manifest",
            ));
        }

        let credential = sqlx::query("SELECT platform_id, scope FROM credentials WHERE id = ?")
            .bind(record.credential_id.to_string())
            .fetch_optional(&mut *transaction)
            .await?
            .ok_or(StorageError::NotFound)?;
        let credential_platform: String = credential.try_get("platform_id")?;
        let credential_scope: String = credential.try_get("scope")?;
        if credential_platform != plugin_platform || credential_scope != record.scope.as_str() {
            return Err(invalid_data(
                "credential grant platform or scope does not match",
            ));
        }

        let credential_origins =
            load_credential_origins_from_transaction(&mut transaction, record.credential_id)
                .await?;
        let declared_origins = content_scope_origins(&manifest_json, &record.scope)?
            .ok_or_else(|| invalid_data("content plugin does not declare the credential scope"))?;
        let credential_allowed = credential_origins
            .iter()
            .map(CredentialTargetOrigin::as_str)
            .collect::<HashSet<_>>();
        let manifest_allowed = declared_origins
            .iter()
            .map(CredentialTargetOrigin::as_str)
            .collect::<HashSet<_>>();
        if grant_origins.iter().any(|origin| {
            !credential_allowed.contains(origin.as_str())
                || !manifest_allowed.contains(origin.as_str())
        }) {
            return Err(invalid_data(
                "credential grant origins must be in the declared credential intersection",
            ));
        }

        sqlx::query(
            r#"
            UPDATE credential_scope_grants
            SET revoked_at = ?
            WHERE plugin_id = ? AND credential_id = ? AND scope = ?
              AND revoked_at IS NULL
            "#,
        )
        .bind(record.created_at.to_rfc3339())
        .bind(record.plugin_id.as_str())
        .bind(record.credential_id.to_string())
        .bind(record.scope.as_str())
        .execute(&mut *transaction)
        .await?;

        sqlx::query(
            r#"
            INSERT INTO credential_scope_grants (
              id, plugin_id, manifest_hash, credential_id, scope,
              credential_origins_hash, created_at, revoked_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, NULL)
            "#,
        )
        .bind(record.id.to_string())
        .bind(record.plugin_id.as_str())
        .bind(&record.manifest_hash)
        .bind(record.credential_id.to_string())
        .bind(record.scope.as_str())
        .bind(origin_set_hash(&credential_origins).as_slice())
        .bind(record.created_at.to_rfc3339())
        .execute(&mut *transaction)
        .await
        .map_err(map_write_error)?;

        insert_grant_origins(
            &mut transaction,
            record.id,
            record.credential_id,
            &grant_origins,
        )
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn revoke_grant(
        &self,
        grant_id: Uuid,
        revoked_at: DateTime<Utc>,
    ) -> Result<(), StorageError> {
        let result = sqlx::query(
            "UPDATE credential_scope_grants SET revoked_at = ? WHERE id = ? AND revoked_at IS NULL",
        )
        .bind(revoked_at.to_rfc3339())
        .bind(grant_id.to_string())
        .execute(self.pool)
        .await?;
        ensure_one_row(result.rows_affected())
    }

    pub async fn list_grants_for_plugin(
        &self,
        plugin_id: &PluginId,
    ) -> Result<Vec<CredentialScopeGrantRecord>, StorageError> {
        let mut transaction = self.pool.begin().await?;
        let rows = sqlx::query(
            r#"
            SELECT id, plugin_id, manifest_hash, credential_id, scope, created_at, revoked_at
            FROM credential_scope_grants
            WHERE plugin_id = ?
            ORDER BY created_at ASC, id ASC
            "#,
        )
        .bind(plugin_id.as_str())
        .fetch_all(&mut *transaction)
        .await?;
        let mut grants = Vec::with_capacity(rows.len());
        for row in rows {
            let mut grant = decode_grant(row)?;
            grant.target_origins =
                load_grant_origins_from_transaction(&mut transaction, grant.id).await?;
            grants.push(grant);
        }
        transaction.commit().await?;
        Ok(grants)
    }

    pub async fn active_grant(
        &self,
        plugin_id: &PluginId,
        credential_id: &CredentialId,
        scope: &CredentialScope,
    ) -> Result<Option<CredentialScopeGrantRecord>, StorageError> {
        let mut transaction = self.pool.begin().await?;
        let row = sqlx::query(
            r#"
            SELECT
              g.id, g.plugin_id, g.manifest_hash, g.credential_id, g.scope,
              g.credential_origins_hash, g.created_at, g.revoked_at,
              p.manifest_json
            FROM credential_scope_grants g
            JOIN plugins p ON p.plugin_id = g.plugin_id
            JOIN credentials c ON c.id = g.credential_id
            WHERE g.plugin_id = ? AND g.credential_id = ? AND g.scope = ?
              AND g.revoked_at IS NULL
              AND p.plugin_type = 'content'
              AND p.platform_id = c.platform_id
              AND p.manifest_hash = g.manifest_hash
              AND c.scope = g.scope
            "#,
        )
        .bind(plugin_id.as_str())
        .bind(credential_id.to_string())
        .bind(scope.as_str())
        .fetch_optional(&mut *transaction)
        .await?;
        let Some(row) = row else {
            transaction.commit().await?;
            return Ok(None);
        };

        let stored_hash = decode_origin_hash(row.try_get("credential_origins_hash")?)?;
        let manifest_json: String = row.try_get("manifest_json")?;
        let mut grant = decode_grant(row)?;
        grant.target_origins =
            load_grant_origins_from_transaction(&mut transaction, grant.id).await?;
        let credential_origins =
            load_credential_origins_from_transaction(&mut transaction, grant.credential_id).await?;
        let Some(declared_origins) = content_scope_origins(&manifest_json, &grant.scope)? else {
            transaction.commit().await?;
            return Ok(None);
        };
        if grant.target_origins.is_empty() || stored_hash != origin_set_hash(&credential_origins) {
            transaction.commit().await?;
            return Ok(None);
        }
        let credential_allowed = credential_origins
            .iter()
            .map(CredentialTargetOrigin::as_str)
            .collect::<HashSet<_>>();
        let manifest_allowed = declared_origins
            .iter()
            .map(CredentialTargetOrigin::as_str)
            .collect::<HashSet<_>>();
        if grant.target_origins.iter().any(|origin| {
            !credential_allowed.contains(origin.as_str())
                || !manifest_allowed.contains(origin.as_str())
        }) {
            transaction.commit().await?;
            return Ok(None);
        }
        transaction.commit().await?;
        Ok(Some(grant))
    }
}

async fn insert_credential(
    transaction: &mut Transaction<'_, Sqlite>,
    record: &CredentialRecord,
) -> Result<(), StorageError> {
    sqlx::query(
        r#"
        INSERT INTO credentials (
          id, kind, platform_id, scope, source_plugin_id, algorithm_version,
          key_version, nonce, ciphertext, status, account_id_hint, display_name,
          safe_error_summary, expires_at, status_checked_at, record_revision,
          created_at, updated_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(record.id.to_string())
    .bind(credential_kind_to_str(record.kind))
    .bind(&record.platform_id)
    .bind(record.scope.as_str())
    .bind(record.source_plugin_id.as_ref().map(PluginId::as_str))
    .bind(i64::from(record.algorithm_version))
    .bind(i64::from(record.key_version))
    .bind(record.nonce.as_slice())
    .bind(&record.ciphertext)
    .bind(credential_status_to_str(record.status))
    .bind(&record.account_id_hint)
    .bind(&record.display_name)
    .bind(&record.safe_error_summary)
    .bind(record.expires_at.map(|timestamp| timestamp.to_rfc3339()))
    .bind(
        record
            .status_checked_at
            .map(|timestamp| timestamp.to_rfc3339()),
    )
    .bind(u64_to_i64(record.revision)?)
    .bind(record.created_at.to_rfc3339())
    .bind(record.updated_at.to_rfc3339())
    .execute(&mut **transaction)
    .await
    .map_err(map_write_error)?;
    Ok(())
}

async fn validate_source_plugin(
    transaction: &mut Transaction<'_, Sqlite>,
    record: &CredentialRecord,
) -> Result<(), StorageError> {
    let Some(source_plugin_id) = &record.source_plugin_id else {
        return Ok(());
    };
    let row = sqlx::query("SELECT plugin_type, platform_id FROM plugins WHERE plugin_id = ?")
        .bind(source_plugin_id.as_str())
        .fetch_optional(&mut **transaction)
        .await?
        .ok_or(StorageError::NotFound)?;
    let plugin_type: String = row.try_get("plugin_type")?;
    let platform_id: String = row.try_get("platform_id")?;
    if plugin_type != "credential" || platform_id != record.platform_id {
        return Err(invalid_data(
            "credential source must be a credential plugin for the same platform",
        ));
    }
    Ok(())
}

async fn replace_credential_origins(
    transaction: &mut Transaction<'_, Sqlite>,
    credential_id: CredentialId,
    origins: &[CredentialTargetOrigin],
) -> Result<(), StorageError> {
    sqlx::query("DELETE FROM credential_target_origins WHERE credential_id = ?")
        .bind(credential_id.to_string())
        .execute(&mut **transaction)
        .await?;
    for origin in origins {
        sqlx::query("INSERT INTO credential_target_origins (credential_id, origin) VALUES (?, ?)")
            .bind(credential_id.to_string())
            .bind(origin.as_str())
            .execute(&mut **transaction)
            .await
            .map_err(map_write_error)?;
    }
    Ok(())
}

async fn load_credential_origins_from_transaction(
    transaction: &mut Transaction<'_, Sqlite>,
    credential_id: CredentialId,
) -> Result<Vec<CredentialTargetOrigin>, StorageError> {
    let rows = sqlx::query(
        "SELECT origin FROM credential_target_origins WHERE credential_id = ? ORDER BY origin ASC",
    )
    .bind(credential_id.to_string())
    .fetch_all(&mut **transaction)
    .await?;
    decode_origins(rows)
}

async fn insert_grant_origins(
    transaction: &mut Transaction<'_, Sqlite>,
    grant_id: Uuid,
    credential_id: CredentialId,
    origins: &[CredentialTargetOrigin],
) -> Result<(), StorageError> {
    for origin in origins {
        sqlx::query(
            r#"
            INSERT INTO credential_scope_grant_origins (
              grant_id, credential_id, origin
            ) VALUES (?, ?, ?)
            "#,
        )
        .bind(grant_id.to_string())
        .bind(credential_id.to_string())
        .bind(origin.as_str())
        .execute(&mut **transaction)
        .await
        .map_err(map_write_error)?;
    }
    Ok(())
}

async fn load_grant_origins_from_transaction(
    transaction: &mut Transaction<'_, Sqlite>,
    grant_id: Uuid,
) -> Result<Vec<CredentialTargetOrigin>, StorageError> {
    let rows = sqlx::query(
        "SELECT origin FROM credential_scope_grant_origins WHERE grant_id = ? ORDER BY origin ASC",
    )
    .bind(grant_id.to_string())
    .fetch_all(&mut **transaction)
    .await?;
    decode_origins(rows)
}

fn decode_credential(row: sqlx::sqlite::SqliteRow) -> Result<CredentialRecord, StorageError> {
    let id: String = row.try_get("id")?;
    let kind: String = row.try_get("kind")?;
    let scope: String = row.try_get("scope")?;
    let source_plugin_id: Option<String> = row.try_get("source_plugin_id")?;
    let algorithm_version: i64 = row.try_get("algorithm_version")?;
    let key_version: i64 = row.try_get("key_version")?;
    let nonce: Vec<u8> = row.try_get("nonce")?;
    let ciphertext: Vec<u8> = row.try_get("ciphertext")?;
    let status: String = row.try_get("status")?;
    let expires_at: Option<String> = row.try_get("expires_at")?;
    let status_checked_at: Option<String> = row.try_get("status_checked_at")?;
    let revision: i64 = row.try_get("record_revision")?;
    let created_at: String = row.try_get("created_at")?;
    let updated_at: String = row.try_get("updated_at")?;

    if !(MIN_CIPHERTEXT_BYTES..=MAX_CIPHERTEXT_BYTES).contains(&ciphertext.len()) {
        return Err(invalid_data("stored credential ciphertext is malformed"));
    }

    Ok(CredentialRecord {
        id: CredentialId::parse(id)
            .map_err(|_| invalid_data("stored credential ID is malformed"))?,
        kind: credential_kind_from_str(&kind)?,
        platform_id: row.try_get("platform_id")?,
        scope: CredentialScope::parse(scope)
            .map_err(|_| invalid_data("stored credential scope is malformed"))?,
        source_plugin_id: source_plugin_id
            .map(|plugin_id| {
                PluginId::parse(plugin_id)
                    .map_err(|_| invalid_data("stored source plugin ID is malformed"))
            })
            .transpose()?,
        algorithm_version: u16::try_from(algorithm_version)
            .ok()
            .filter(|version| *version > 0)
            .ok_or_else(|| invalid_data("stored algorithm version is malformed"))?,
        key_version: u32::try_from(key_version)
            .ok()
            .filter(|version| *version > 0)
            .ok_or_else(|| invalid_data("stored key version is malformed"))?,
        nonce: nonce
            .try_into()
            .map_err(|_| invalid_data("stored credential nonce is malformed"))?,
        ciphertext,
        target_origins: Vec::new(),
        status: credential_status_from_str(&status)?,
        account_id_hint: row.try_get("account_id_hint")?,
        display_name: row.try_get("display_name")?,
        safe_error_summary: row.try_get("safe_error_summary")?,
        expires_at: parse_optional_timestamp(expires_at)?,
        status_checked_at: parse_optional_timestamp(status_checked_at)?,
        revision: i64_to_u64(revision, "stored credential revision is malformed")?,
        created_at: parse_timestamp(created_at)?,
        updated_at: parse_timestamp(updated_at)?,
    })
}

fn decode_grant(row: sqlx::sqlite::SqliteRow) -> Result<CredentialScopeGrantRecord, StorageError> {
    let id: String = row.try_get("id")?;
    let plugin_id: String = row.try_get("plugin_id")?;
    let credential_id: String = row.try_get("credential_id")?;
    let scope: String = row.try_get("scope")?;
    let created_at: String = row.try_get("created_at")?;
    let revoked_at: Option<String> = row.try_get("revoked_at")?;
    Ok(CredentialScopeGrantRecord {
        id: Uuid::parse_str(&id)
            .map_err(|_| invalid_data("stored credential grant ID is malformed"))?,
        plugin_id: PluginId::parse(plugin_id)
            .map_err(|_| invalid_data("stored grant plugin ID is malformed"))?,
        manifest_hash: row.try_get("manifest_hash")?,
        credential_id: CredentialId::parse(credential_id)
            .map_err(|_| invalid_data("stored grant credential ID is malformed"))?,
        scope: CredentialScope::parse(scope)
            .map_err(|_| invalid_data("stored grant scope is malformed"))?,
        target_origins: Vec::new(),
        created_at: parse_timestamp(created_at)?,
        revoked_at: parse_optional_timestamp(revoked_at)?,
    })
}

fn decode_origins(
    rows: Vec<sqlx::sqlite::SqliteRow>,
) -> Result<Vec<CredentialTargetOrigin>, StorageError> {
    let mut origins = Vec::with_capacity(rows.len());
    for row in rows {
        let origin: String = row.try_get("origin")?;
        origins.push(
            CredentialTargetOrigin::parse(origin)
                .map_err(|_| invalid_data("stored credential origin is malformed"))?,
        );
    }
    Ok(origins)
}

fn validate_credential_record(record: &CredentialRecord) -> Result<(), StorageError> {
    validate_platform_id(&record.platform_id)?;
    if record
        .scope
        .as_str()
        .split_once('.')
        .is_none_or(|(platform, _)| platform != record.platform_id)
    {
        return Err(invalid_data(
            "credential scope must belong to the credential platform",
        ));
    }
    if record.algorithm_version == 0
        || record.key_version == 0
        || record.revision == 0
        || !(MIN_CIPHERTEXT_BYTES..=MAX_CIPHERTEXT_BYTES).contains(&record.ciphertext.len())
        || record.updated_at < record.created_at
    {
        return Err(invalid_data("credential encrypted record is invalid"));
    }
    validate_safe_metadata(&record.account_id_hint, 256)?;
    validate_safe_metadata(&record.display_name, 256)?;
    validate_safe_metadata(&record.safe_error_summary, MAX_SAFE_METADATA_BYTES)?;
    Ok(())
}

fn validate_grant_record(record: &CredentialScopeGrantRecord) -> Result<(), StorageError> {
    if record.id.is_nil()
        || record.manifest_hash.is_empty()
        || record.manifest_hash.len() > 256
        || record.revoked_at.is_some()
    {
        return Err(invalid_data("credential grant record is invalid"));
    }
    Ok(())
}

fn validate_safe_metadata(value: &Option<String>, max_bytes: usize) -> Result<(), StorageError> {
    if value
        .as_ref()
        .is_some_and(|value| value.is_empty() || value.len() > max_bytes)
    {
        Err(invalid_data("credential safe metadata is invalid"))
    } else {
        Ok(())
    }
}

fn normalize_origins(
    origins: &[CredentialTargetOrigin],
) -> Result<Vec<CredentialTargetOrigin>, StorageError> {
    if origins.is_empty() || origins.len() > MAX_CREDENTIAL_TARGET_ORIGINS {
        return Err(invalid_data("credential origin set is invalid"));
    }
    let mut normalized = origins.to_vec();
    normalized.sort_by(|left, right| left.as_str().cmp(right.as_str()));
    if normalized
        .windows(2)
        .any(|pair| pair[0].as_str() == pair[1].as_str())
    {
        return Err(invalid_data("credential origin set contains duplicates"));
    }
    Ok(normalized)
}

fn content_scope_origins(
    manifest_json: &str,
    scope: &CredentialScope,
) -> Result<Option<Vec<CredentialTargetOrigin>>, StorageError> {
    let manifest: serde_json::Value = serde_json::from_str(manifest_json)
        .map_err(|_| invalid_data("stored plugin manifest is malformed"))?;
    let credentials = match manifest.get("credentials") {
        Some(value) => serde_json::from_value::<CredentialDeclarations>(value.clone())
            .map_err(|_| invalid_data("stored credential declarations are malformed"))?,
        None => CredentialDeclarations::default(),
    };
    credentials
        .required_scopes
        .iter()
        .chain(&credentials.optional_scopes)
        .find(|declaration| declaration.scope == *scope)
        .map(|declaration| normalize_origins(&declaration.target_origins))
        .transpose()
}

fn origin_set_hash(origins: &[CredentialTargetOrigin]) -> [u8; ORIGIN_HASH_BYTES] {
    let mut hasher = Sha256::new();
    hasher.update(b"audiodown-credential-origins\0");
    for origin in origins {
        let bytes = origin.as_str().as_bytes();
        hasher.update((bytes.len() as u32).to_be_bytes());
        hasher.update(bytes);
    }
    hasher.finalize().into()
}

fn decode_origin_hash(value: Vec<u8>) -> Result<[u8; ORIGIN_HASH_BYTES], StorageError> {
    value
        .try_into()
        .map_err(|_| invalid_data("stored credential origin binding is malformed"))
}

fn credential_kind_to_str(kind: CredentialKind) -> &'static str {
    match kind {
        CredentialKind::Cookie => "cookie",
        CredentialKind::Token => "token",
    }
}

fn credential_kind_from_str(value: &str) -> Result<CredentialKind, StorageError> {
    match value {
        "cookie" => Ok(CredentialKind::Cookie),
        "token" => Ok(CredentialKind::Token),
        _ => Err(invalid_data("stored credential kind is malformed")),
    }
}

fn credential_status_to_str(status: CredentialStatus) -> &'static str {
    match status {
        CredentialStatus::Active => "active",
        CredentialStatus::Expired => "expired",
        CredentialStatus::Revoked => "revoked",
        CredentialStatus::Error => "error",
    }
}

fn credential_status_from_str(value: &str) -> Result<CredentialStatus, StorageError> {
    match value {
        "active" => Ok(CredentialStatus::Active),
        "expired" => Ok(CredentialStatus::Expired),
        "revoked" => Ok(CredentialStatus::Revoked),
        "error" => Ok(CredentialStatus::Error),
        _ => Err(invalid_data("stored credential status is malformed")),
    }
}

fn validate_platform_id(platform_id: &str) -> Result<(), StorageError> {
    if platform_id.is_empty()
        || platform_id.len() > 128
        || !platform_id.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        })
    {
        Err(invalid_data("credential platform ID is invalid"))
    } else {
        Ok(())
    }
}

fn parse_timestamp(value: String) -> Result<DateTime<Utc>, StorageError> {
    DateTime::parse_from_rfc3339(&value)
        .map(|timestamp| timestamp.with_timezone(&Utc))
        .map_err(|_| invalid_data("stored credential timestamp is malformed"))
}

fn parse_optional_timestamp(value: Option<String>) -> Result<Option<DateTime<Utc>>, StorageError> {
    value.map(parse_timestamp).transpose()
}

fn u64_to_i64(value: u64) -> Result<i64, StorageError> {
    i64::try_from(value).map_err(|_| invalid_data("credential revision is invalid"))
}

fn i64_to_u64(value: i64, message: &'static str) -> Result<u64, StorageError> {
    u64::try_from(value)
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| invalid_data(message))
}

fn ensure_one_row(rows_affected: u64) -> Result<(), StorageError> {
    if rows_affected == 1 {
        Ok(())
    } else {
        Err(StorageError::NotFound)
    }
}

fn map_write_error(error: sqlx::Error) -> StorageError {
    match &error {
        sqlx::Error::Database(database_error) if database_error.is_unique_violation() => {
            StorageError::Conflict
        }
        _ => StorageError::Database(error),
    }
}

fn invalid_data(message: &'static str) -> StorageError {
    StorageError::InvalidData(message.to_string())
}
