use audiodown_domain::{
    log::{LogLevel, StructuredLog},
    plugin::{PluginId, PluginStatus, RunMode},
};
use audiodown_plugin_api::manifest::PluginType;
use audiodown_storage::{PluginRecord, RiskGrantRecord, Storage, StorageError};
use chrono::{Duration, Utc};
use uuid::Uuid;

#[tokio::test]
async fn persists_installation_settings_and_risk_grants() -> Result<(), Box<dyn std::error::Error>>
{
    let storage = Storage::connect("sqlite::memory:").await?;
    storage.migrate().await?;
    let plugin_id = PluginId::parse("com.audiodown.virtual.content")?;
    let now = Utc::now();
    storage
        .plugins()
        .upsert(&fixture_record(&plugin_id))
        .await?;

    let operation_id = Uuid::new_v4();
    storage
        .plugins()
        .set_install_result(
            &plugin_id,
            "example.plugins",
            "sha256:image",
            "source-tree-sha256",
            operation_id,
            PluginStatus::Installing,
        )
        .await?;
    storage
        .plugins()
        .update_settings(&plugin_id, false, RunMode::Always, 25)
        .await?;
    storage.plugins().touch(&plugin_id, now).await?;

    let plugin = storage.plugins().get(&plugin_id).await?.unwrap();
    assert_eq!(plugin.repository_id.as_deref(), Some("example.plugins"));
    assert_eq!(plugin.image_id.as_deref(), Some("sha256:image"));
    assert_eq!(plugin.source_hash.as_deref(), Some("source-tree-sha256"));
    assert_eq!(plugin.install_operation_id, Some(operation_id));
    assert_eq!(plugin.status, PluginStatus::Installing);
    assert_eq!(plugin.run_mode, RunMode::Always);
    assert_eq!(plugin.priority, 25);
    assert!(!plugin.enabled);
    assert_eq!(plugin.last_used_at, Some(now));

    let pending = storage.plugins().list_pending_install_operations().await?;
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].install_operation_id, Some(operation_id));

    let grant = RiskGrantRecord {
        id: Uuid::new_v4(),
        repository_id: "example.plugins".into(),
        plugin_id: plugin_id.clone(),
        commit_sha: "0123456789abcdef0123456789abcdef01234567".into(),
        risk_kind: "npm_lifecycle_scripts".into(),
        reason: "Generate a deterministic local file".into(),
        granted_at: now,
    };
    storage.risk_grants().insert(&grant).await?;
    assert!(
        storage
            .risk_grants()
            .exists_for(&plugin_id, &grant.commit_sha, "npm_lifecycle_scripts")
            .await?
    );

    assert!(matches!(
        storage
            .plugins()
            .complete_install(&plugin_id, Uuid::new_v4())
            .await,
        Err(StorageError::NotFound)
    ));
    storage
        .plugins()
        .complete_install(&plugin_id, operation_id)
        .await?;
    let installed = storage.plugins().get(&plugin_id).await?.unwrap();
    assert_eq!(installed.status, PluginStatus::Installed);
    assert_eq!(installed.install_operation_id, None);

    storage.plugins().delete(&plugin_id).await?;
    assert!(storage.plugins().get(&plugin_id).await?.is_none());
    assert!(
        storage
            .risk_grants()
            .exists_for(&plugin_id, &grant.commit_sha, "npm_lifecycle_scripts")
            .await?
    );

    Ok(())
}

#[tokio::test]
async fn conditionally_rolls_back_only_the_current_install_operation(
) -> Result<(), Box<dyn std::error::Error>> {
    let storage = Storage::connect("sqlite::memory:").await?;
    storage.migrate().await?;
    let plugin_id = PluginId::parse("com.audiodown.virtual.rollback")?;
    storage
        .plugins()
        .upsert(&fixture_record(&plugin_id))
        .await?;
    let operation_id = Uuid::new_v4();
    storage
        .plugins()
        .set_install_result(
            &plugin_id,
            "example.plugins",
            "sha256:image",
            "source-tree-sha256",
            operation_id,
            PluginStatus::Installing,
        )
        .await?;

    assert!(matches!(
        storage
            .plugins()
            .rollback_install(&plugin_id, Uuid::new_v4())
            .await,
        Err(StorageError::NotFound)
    ));
    assert!(storage.plugins().get(&plugin_id).await?.is_some());

    storage
        .plugins()
        .rollback_install(&plugin_id, operation_id)
        .await?;
    assert!(storage.plugins().get(&plugin_id).await?.is_none());
    Ok(())
}

#[tokio::test]
async fn rejects_invalid_priorities_and_missing_plugin_mutations(
) -> Result<(), Box<dyn std::error::Error>> {
    let storage = Storage::connect("sqlite::memory:").await?;
    storage.migrate().await?;
    let missing = PluginId::parse("com.audiodown.virtual.missing")?;

    for priority in [-1, 1_001] {
        assert!(matches!(
            storage
                .plugins()
                .update_settings(&missing, true, RunMode::OnDemand, priority)
                .await,
            Err(StorageError::InvalidData(_))
        ));
    }

    assert!(matches!(
        storage
            .plugins()
            .set_install_result(
                &missing,
                "example.plugins",
                "sha256:image",
                "source-tree-sha256",
                Uuid::new_v4(),
                PluginStatus::Installing,
            )
            .await,
        Err(StorageError::NotFound)
    ));
    assert!(matches!(
        storage
            .plugins()
            .update_settings(&missing, true, RunMode::OnDemand, 100)
            .await,
        Err(StorageError::NotFound)
    ));
    assert!(matches!(
        storage.plugins().touch(&missing, Utc::now()).await,
        Err(StorageError::NotFound)
    ));
    assert!(matches!(
        storage.plugins().delete(&missing).await,
        Err(StorageError::NotFound)
    ));
    Ok(())
}

#[tokio::test]
async fn insert_installing_never_overwrites_an_existing_plugin(
) -> Result<(), Box<dyn std::error::Error>> {
    let storage = Storage::connect("sqlite::memory:").await?;
    storage.migrate().await?;
    let plugin_id = PluginId::parse("com.audiodown.virtual.atomic")?;
    let mut existing = fixture_record(&plugin_id);
    existing.install_operation_id = Some(Uuid::new_v4());
    existing.status = PluginStatus::Installing;
    storage.plugins().upsert(&existing).await?;

    let mut replacement = fixture_record(&plugin_id);
    replacement.install_operation_id = Some(Uuid::new_v4());
    replacement.status = PluginStatus::Installing;
    assert!(storage
        .plugins()
        .insert_installing(&replacement)
        .await
        .is_err());
    let stored = storage.plugins().get(&plugin_id).await?.unwrap();
    assert_eq!(stored.install_operation_id, existing.install_operation_id);
    Ok(())
}

#[tokio::test]
async fn replaces_commit_specific_risk_audit_after_fresh_authorization(
) -> Result<(), Box<dyn std::error::Error>> {
    let storage = Storage::connect("sqlite::memory:").await?;
    storage.migrate().await?;
    let plugin_id = PluginId::parse("com.audiodown.virtual.risk")?;
    let first = RiskGrantRecord {
        id: Uuid::new_v4(),
        repository_id: "example.plugins".into(),
        plugin_id: plugin_id.clone(),
        commit_sha: "0123456789abcdef0123456789abcdef01234567".into(),
        risk_kind: "npm_lifecycle_scripts".into(),
        reason: "first approval".into(),
        granted_at: Utc::now(),
    };
    storage.risk_grants().insert(&first).await?;
    let replacement = RiskGrantRecord {
        id: Uuid::new_v4(),
        reason: "fresh approval".into(),
        ..first.clone()
    };

    storage
        .risk_grants()
        .replace_commit_grant(&replacement)
        .await?;
    let stored = storage
        .risk_grants()
        .get_for(&plugin_id, &replacement.commit_sha, &replacement.risk_kind)
        .await?
        .unwrap();
    assert_eq!(stored.id, replacement.id);
    assert_eq!(stored.reason, "fresh approval");
    Ok(())
}

#[tokio::test]
async fn build_log_replay_is_idempotent() -> Result<(), Box<dyn std::error::Error>> {
    let storage = Storage::connect("sqlite::memory:").await?;
    storage.migrate().await?;
    let log = StructuredLog {
        id: Uuid::new_v5(&Uuid::NAMESPACE_OID, b"operation:1"),
        timestamp: Utc::now(),
        level: LogLevel::Info,
        component: "plugin-build".into(),
        message: "deterministic build output".into(),
        plugin_id: Some("com.audiodown.virtual.content".into()),
        plugin_version: Some("1.0.0".into()),
        platform_id: Some("virtual".into()),
        request_id: Some(Uuid::new_v4().to_string()),
        task_id: None,
        container_id: None,
        error_code: None,
        context: serde_json::json!({"sequence": 1}),
    };

    storage.logs().append_if_absent(&log).await?;
    storage.logs().append_if_absent(&log).await?;
    assert_eq!(
        storage
            .logs()
            .list(audiodown_storage::LogFilter {
                plugin_id: None,
                limit: 10,
            })
            .await?
            .len(),
        1
    );
    Ok(())
}

fn fixture_record(plugin_id: &PluginId) -> PluginRecord {
    let now = Utc::now() - Duration::seconds(1);
    PluginRecord {
        plugin_id: plugin_id.clone(),
        plugin_type: PluginType::Content,
        platform_id: "virtual".to_string(),
        name: "Virtual Content".to_string(),
        version: "1.0.0".to_string(),
        protocol_version: "1.0".to_string(),
        source_kind: "github".to_string(),
        source_ref: "https://github.com/example-owner/example-repository".to_string(),
        commit_sha: Some("0123456789abcdef0123456789abcdef01234567".to_string()),
        repository_id: None,
        manifest_json: serde_json::json!({"id": plugin_id.as_str()}),
        manifest_hash: "manifest-sha256".to_string(),
        source_hash: None,
        image_id: None,
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
