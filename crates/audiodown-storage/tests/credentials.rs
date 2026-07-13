use audiodown_credential_vault::{encrypt, EncryptionContext, MasterKey};
use audiodown_domain::{
    credential::{CredentialId, CredentialKind, CredentialScope, CredentialStatus},
    plugin::{PluginId, PluginStatus, RunMode},
};
use audiodown_plugin_api::manifest::{CredentialTargetOrigin, PluginType};
use audiodown_storage::{
    CredentialRecord, CredentialScopeGrantRecord, PluginRecord, Storage, StorageError,
};
use chrono::{Duration, TimeZone, Utc};
use secrecy::{Secret, SecretVec};
use sqlx::{Connection, Row, SqliteConnection, SqlitePool};
use tempfile::{tempdir, TempDir};
use tokio::sync::Barrier;
use uuid::Uuid;

const PLAINTEXT_CANARY: &str = "credential-plaintext-canary-must-never-persist";

#[tokio::test]
async fn persists_encrypted_credentials_and_safe_metadata_without_plaintext(
) -> Result<(), Box<dyn std::error::Error>> {
    let (_temporary, database_path, database_url, storage) = migrated_file_storage().await?;
    let inspection_pool = SqlitePool::connect(&database_url).await?;
    let owner = PluginId::parse("com.audiodown.virtual.credential")?;
    storage
        .plugins()
        .upsert(&plugin_record(
            &owner,
            PluginType::Credential,
            "virtual",
            "credential-manifest-v1",
        ))
        .await?;

    let mut record = credential_record(&owner);
    storage.credentials().insert(&record).await?;

    let stored = storage.credentials().get(&record.id).await?.unwrap();
    assert_eq!(stored, record);
    assert_eq!(
        stored.target_origins[0].as_str(),
        "https://account.virtual.invalid"
    );
    assert_eq!(
        storage
            .credentials()
            .get_by_scope(&record.scope)
            .await?
            .unwrap(),
        record
    );
    assert_eq!(storage.credentials().list().await?, vec![record.clone()]);

    let metadata = stored.public_metadata();
    assert_eq!(metadata.id, record.id);
    assert_eq!(metadata.kind, CredentialKind::Cookie);
    assert_eq!(metadata.status, CredentialStatus::Active);
    assert_eq!(
        metadata.ownership.source_plugin_id(),
        Some(&record.source_plugin_id.clone().unwrap())
    );
    let rendered = format!("{stored:?}\n{metadata:?}");
    assert!(!rendered.contains(PLAINTEXT_CANARY));
    assert!(!rendered.contains(&format!("{:?}", record.nonce)));
    assert!(!rendered.contains(&format!("{:?}", record.ciphertext)));

    let mut stale = record.clone();
    record.kind = CredentialKind::Token;
    record.key_version = 2;
    record.nonce = [0x33; 12];
    record.ciphertext = vec![0x44; 48];
    record.status = CredentialStatus::Expired;
    record.account_id_hint = Some("virtual-account".to_string());
    record.display_name = Some("Virtual Account".to_string());
    record.safe_error_summary = Some("credential expired".to_string());
    record.expires_at = Some(Utc::now() - Duration::minutes(1));
    record.status_checked_at = Some(Utc::now());
    record.updated_at = Utc::now();
    record.revision = storage.credentials().upsert(&record).await?;
    assert_eq!(
        storage.credentials().get(&record.id).await?.unwrap(),
        record
    );
    stale.updated_at = Utc::now();
    assert!(matches!(
        storage.credentials().upsert(&stale).await,
        Err(StorageError::Conflict)
    ));

    let mut conflicting = record.clone();
    conflicting.id = CredentialId::parse("bc38834d-1028-4ff8-b24a-3df3d0f179ec")?;
    conflicting.revision = 1;
    assert!(matches!(
        storage.credentials().insert(&conflicting).await,
        Err(StorageError::Conflict)
    ));

    let rows = sqlx::query(
        r#"
        SELECT
          id, kind, platform_id, scope, COALESCE(source_plugin_id, ''),
          status, COALESCE(account_id_hint, ''), COALESCE(display_name, ''),
          COALESCE(safe_error_summary, ''), COALESCE(expires_at, ''),
          COALESCE(status_checked_at, ''), created_at, updated_at
        FROM credentials
        "#,
    )
    .fetch_all(&inspection_pool)
    .await?;
    let all_text = rows
        .iter()
        .flat_map(|row| (0..13).map(|index| row.get::<String, _>(index)))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!all_text.contains(PLAINTEXT_CANARY));
    let schema = sqlx::query_scalar::<_, String>(
        r#"
        SELECT GROUP_CONCAT(sql, char(10))
        FROM sqlite_master
        WHERE name IN (
          'credentials',
          'credential_target_origins',
          'credential_scope_grants',
          'credential_scope_grant_origins'
        )
        "#,
    )
    .fetch_one(&inspection_pool)
    .await?;
    assert!(!schema.contains(PLAINTEXT_CANARY));
    let ciphertexts = sqlx::query_scalar::<_, Vec<u8>>("SELECT ciphertext FROM credentials")
        .fetch_all(&inspection_pool)
        .await?;
    assert!(ciphertexts.iter().all(|ciphertext| {
        !ciphertext
            .windows(PLAINTEXT_CANARY.len())
            .any(|window| window == PLAINTEXT_CANARY.as_bytes())
    }));
    for path in [
        database_path.clone(),
        std::path::PathBuf::from(format!("{}-wal", database_path.display())),
    ] {
        if path.exists() {
            let bytes = std::fs::read(path)?;
            assert!(!bytes
                .windows(PLAINTEXT_CANARY.len())
                .any(|window| window == PLAINTEXT_CANARY.as_bytes()));
        }
    }

    storage.credentials().delete(&record.id).await?;
    assert!(storage.credentials().get(&record.id).await?.is_none());
    Ok(())
}

#[tokio::test]
async fn requires_explicit_retention_before_deleting_an_owner_plugin(
) -> Result<(), Box<dyn std::error::Error>> {
    let storage = migrated_storage().await?;
    let owner = PluginId::parse("com.audiodown.virtual.owner")?;
    storage
        .plugins()
        .upsert(&plugin_record(
            &owner,
            PluginType::Credential,
            "virtual",
            "owner-manifest",
        ))
        .await?;
    let record = credential_record(&owner);
    storage.credentials().insert(&record).await?;

    assert!(storage.plugins().delete(&owner).await.is_err());
    storage
        .credentials()
        .clear_source_plugin(&record.id)
        .await?;
    let retained = storage.credentials().get(&record.id).await?.unwrap();
    assert!(retained.source_plugin_id.is_none());
    assert!(retained.public_metadata().ownership.is_retained());

    storage.plugins().delete(&owner).await?;
    assert!(storage.credentials().get(&record.id).await?.is_some());
    Ok(())
}

#[tokio::test]
async fn grants_require_exact_intersections_and_invalidate_on_binding_changes(
) -> Result<(), Box<dyn std::error::Error>> {
    let storage = migrated_storage().await?;
    let owner = PluginId::parse("com.audiodown.virtual.credential")?;
    let content = PluginId::parse("com.audiodown.virtual.content")?;
    storage
        .plugins()
        .upsert(&plugin_record(
            &owner,
            PluginType::Credential,
            "virtual",
            "credential-manifest",
        ))
        .await?;
    storage
        .plugins()
        .upsert(&plugin_record(
            &content,
            PluginType::Content,
            "virtual",
            "content-manifest-v1",
        ))
        .await?;
    let mut credential = credential_record(&owner);
    storage.credentials().insert(&credential).await?;

    assert!(storage
        .credentials()
        .active_grant(&content, &credential.id, &credential.scope)
        .await?
        .is_none());

    let first = grant_record(
        &content,
        "content-manifest-v1",
        &credential,
        "https://account.virtual.invalid",
    );
    storage.credentials().create_grant(&first).await?;
    assert_eq!(
        storage
            .credentials()
            .active_grant(&content, &credential.id, &credential.scope)
            .await?
            .unwrap(),
        first
    );

    let outside_intersection = grant_record(
        &content,
        "content-manifest-v1",
        &credential,
        "https://media.virtual.invalid",
    );
    assert!(matches!(
        storage
            .credentials()
            .create_grant(&outside_intersection)
            .await,
        Err(StorageError::InvalidData(_))
    ));

    let mut changed_plugin = plugin_record(
        &content,
        PluginType::Content,
        "virtual",
        "content-manifest-v2",
    );
    changed_plugin.updated_at = Utc::now();
    storage.plugins().upsert(&changed_plugin).await?;
    assert!(storage
        .credentials()
        .active_grant(&content, &credential.id, &credential.scope)
        .await?
        .is_none());

    let second = grant_record(
        &content,
        "content-manifest-v2",
        &credential,
        "https://account.virtual.invalid",
    );
    storage.credentials().create_grant(&second).await?;
    assert_eq!(
        storage
            .credentials()
            .active_grant(&content, &credential.id, &credential.scope)
            .await?
            .unwrap()
            .id,
        second.id
    );

    credential.target_origins = vec![CredentialTargetOrigin::parse(
        "https://media.virtual.invalid",
    )?];
    credential.updated_at = Utc::now();
    credential.revision = storage.credentials().upsert(&credential).await?;
    assert!(storage
        .credentials()
        .active_grant(&content, &credential.id, &credential.scope)
        .await?
        .is_none());

    storage
        .credentials()
        .revoke_grant(second.id, Utc::now())
        .await?;
    assert!(storage
        .credentials()
        .active_grant(&content, &credential.id, &credential.scope)
        .await?
        .is_none());

    credential.target_origins = vec![CredentialTargetOrigin::parse(
        "https://account.virtual.invalid",
    )?];
    credential.updated_at = Utc::now();
    credential.revision = storage.credentials().upsert(&credential).await?;
    let third = grant_record(
        &content,
        "content-manifest-v2",
        &credential,
        "https://account.virtual.invalid",
    );
    storage.credentials().create_grant(&third).await?;
    assert_eq!(
        storage
            .credentials()
            .list_grants_for_plugin(&content)
            .await?
            .iter()
            .filter(|grant| grant.revoked_at.is_none())
            .count(),
        1
    );

    storage.plugins().delete(&content).await?;
    assert!(storage
        .credentials()
        .list_grants_for_plugin(&content)
        .await?
        .is_empty());
    Ok(())
}

#[tokio::test]
async fn concurrent_revision_updates_return_one_success_and_one_conflict(
) -> Result<(), Box<dyn std::error::Error>> {
    let (_temporary, _database_path, _database_url, storage) = migrated_file_storage().await?;
    let owner = PluginId::parse("com.audiodown.virtual.concurrent")?;
    storage
        .plugins()
        .upsert(&plugin_record(
            &owner,
            PluginType::Credential,
            "virtual",
            "credential-manifest",
        ))
        .await?;
    let record = credential_record(&owner);
    storage.credentials().insert(&record).await?;

    let barrier = Arc::new(Barrier::new(3));
    let [first_handle, second_handle] = [0x51, 0x61].map(|byte| {
        let storage = storage.clone();
        let barrier = barrier.clone();
        let mut update = record.clone();
        update.ciphertext = vec![byte; 32];
        update.updated_at = Utc::now();
        tokio::spawn(async move {
            barrier.wait().await;
            storage.credentials().upsert(&update).await
        })
    });
    barrier.wait().await;
    let first = first_handle.await?;
    let second = second_handle.await?;

    assert!(
        matches!(
            (&first, &second),
            (Ok(2), Err(StorageError::Conflict)) | (Err(StorageError::Conflict), Ok(2))
        ),
        "unexpected concurrent results: {first:?}, {second:?}"
    );
    Ok(())
}

#[tokio::test]
async fn malformed_rows_return_stable_errors_without_ciphertext(
) -> Result<(), Box<dyn std::error::Error>> {
    let (_temporary, _database_path, database_url, storage) = migrated_file_storage().await?;
    let mut connection = SqliteConnection::connect(&database_url).await?;

    sqlx::query("PRAGMA ignore_check_constraints = ON")
        .execute(&mut connection)
        .await?;
    sqlx::query(
        r#"
        INSERT INTO credentials (
          id, kind, platform_id, scope, source_plugin_id, algorithm_version,
          key_version, nonce, ciphertext, status, expires_at, created_at, updated_at
        ) VALUES (?, ?, ?, ?, NULL, 1, 1, ?, ?, 'active', NULL, ?, ?)
        "#,
    )
    .bind("c241ac0f-0586-45ab-9ac3-e497d76b3378")
    .bind(PLAINTEXT_CANARY)
    .bind("virtual")
    .bind("virtual.web")
    .bind(vec![0x11; 12])
    .bind(PLAINTEXT_CANARY.as_bytes())
    .bind(Utc::now().to_rfc3339())
    .bind(Utc::now().to_rfc3339())
    .execute(&mut connection)
    .await?;

    let id = CredentialId::parse("c241ac0f-0586-45ab-9ac3-e497d76b3378")?;
    let error = storage.credentials().get(&id).await.unwrap_err();
    let rendered = format!("{error:?}\n{error}");
    assert!(matches!(error, StorageError::InvalidData(_)));
    assert!(!rendered.contains(PLAINTEXT_CANARY));
    Ok(())
}

async fn migrated_storage() -> Result<Storage, StorageError> {
    let storage = Storage::connect("sqlite::memory:").await?;
    storage.migrate().await?;
    Ok(storage)
}

async fn migrated_file_storage(
) -> Result<(TempDir, std::path::PathBuf, String, Storage), Box<dyn std::error::Error>> {
    let temporary = tempdir()?;
    let database_path = temporary.path().join("credentials.db");
    let database_url = format!("sqlite://{}", database_path.display());
    let storage = Storage::connect(&database_url).await?;
    storage.migrate().await?;
    Ok((temporary, database_path, database_url, storage))
}

fn credential_record(owner: &PluginId) -> CredentialRecord {
    let created_at = Utc.with_ymd_and_hms(2026, 7, 13, 6, 0, 0).single().unwrap();
    let id = CredentialId::parse("8d86182f-95f7-44d8-a75c-b9d1ec2c18ad").unwrap();
    let scope = CredentialScope::parse("virtual.web").unwrap();
    let key = MasterKey::from_secret(Secret::new([0xA5; 32]));
    let envelope = encrypt(
        &key,
        &EncryptionContext::new(id, scope.clone(), 1),
        &SecretVec::new(PLAINTEXT_CANARY.as_bytes().to_vec()),
    )
    .unwrap();
    CredentialRecord {
        id,
        kind: CredentialKind::Cookie,
        platform_id: "virtual".to_string(),
        scope,
        source_plugin_id: Some(owner.clone()),
        algorithm_version: envelope.algorithm_version(),
        key_version: envelope.key_version(),
        nonce: *envelope.nonce(),
        ciphertext: envelope.ciphertext().to_vec(),
        target_origins: vec![
            CredentialTargetOrigin::parse("HTTPS://ACCOUNT.VIRTUAL.INVALID:443").unwrap(),
            CredentialTargetOrigin::parse("https://media.virtual.invalid").unwrap(),
        ],
        status: CredentialStatus::Active,
        account_id_hint: None,
        display_name: None,
        safe_error_summary: None,
        expires_at: Some(created_at + Duration::hours(1)),
        status_checked_at: None,
        revision: 1,
        created_at,
        updated_at: created_at,
    }
}

fn grant_record(
    plugin_id: &PluginId,
    manifest_hash: &str,
    credential: &CredentialRecord,
    origin: &str,
) -> CredentialScopeGrantRecord {
    CredentialScopeGrantRecord {
        id: Uuid::new_v4(),
        plugin_id: plugin_id.clone(),
        manifest_hash: manifest_hash.to_string(),
        credential_id: credential.id,
        scope: credential.scope.clone(),
        target_origins: vec![CredentialTargetOrigin::parse(origin).unwrap()],
        created_at: Utc::now(),
        revoked_at: None,
    }
}

fn plugin_record(
    plugin_id: &PluginId,
    plugin_type: PluginType,
    platform_id: &str,
    manifest_hash: &str,
) -> PluginRecord {
    let now = Utc::now();
    PluginRecord {
        plugin_id: plugin_id.clone(),
        plugin_type,
        platform_id: platform_id.to_string(),
        name: plugin_id.as_str().to_string(),
        version: "1.0.0".to_string(),
        protocol_version: "1.0".to_string(),
        source_kind: "fixture".to_string(),
        source_ref: "virtual".to_string(),
        commit_sha: None,
        repository_id: None,
        manifest_json: serde_json::json!({
            "id": plugin_id.as_str(),
            "credentials": match plugin_type {
                PluginType::Content => serde_json::json!({
                    "requiredScopes": [{
                        "scope": "virtual.web",
                        "targetOrigins": ["https://account.virtual.invalid"]
                    }]
                }),
                PluginType::Credential => serde_json::json!({
                    "providedScopes": [{
                        "scope": "virtual.web",
                        "targetOrigins": [
                            "https://account.virtual.invalid",
                            "https://media.virtual.invalid"
                        ]
                    }]
                }),
            }
        }),
        manifest_hash: manifest_hash.to_string(),
        source_hash: None,
        image_id: Some("sha256:virtual".to_string()),
        status: PluginStatus::Installed,
        run_mode: RunMode::OnDemand,
        priority: 100,
        enabled: true,
        last_error: None,
        install_operation_id: None,
        last_used_at: None,
        installed_at: now,
        updated_at: now,
    }
}
use std::sync::Arc;
