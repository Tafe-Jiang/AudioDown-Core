use audiodown_domain::plugin::{PluginId, PluginStatus, RunMode};
use audiodown_plugin_api::manifest::PluginType;
use audiodown_storage::{
    ContentParticipation, ContentParticipationKind, PluginRecord, Storage, StorageError,
};
use chrono::Utc;

#[tokio::test]
async fn defaults_content_plugins_to_search_and_discover_participation(
) -> Result<(), Box<dyn std::error::Error>> {
    let storage = migrated_storage().await?;
    let plugin_id = PluginId::parse("com.audiodown.virtual.primary")?;
    storage
        .plugins()
        .upsert(&plugin_record(
            &plugin_id,
            PluginType::Content,
            "virtual",
            100,
        ))
        .await?;

    assert_eq!(
        storage.content_routing().participation(&plugin_id).await?,
        ContentParticipation {
            search_enabled: true,
            discover_enabled: true,
        }
    );
    Ok(())
}

#[tokio::test]
async fn updates_independent_participation_and_orders_default_first(
) -> Result<(), Box<dyn std::error::Error>> {
    let storage = migrated_storage().await?;
    let primary = PluginId::parse("com.audiodown.virtual.primary")?;
    let faster = PluginId::parse("com.audiodown.virtual.faster")?;
    let other = PluginId::parse("com.audiodown.catalog.primary")?;
    for record in [
        plugin_record(&primary, PluginType::Content, "virtual", 100),
        plugin_record(&faster, PluginType::Content, "virtual", 20),
        plugin_record(&other, PluginType::Content, "catalog", 10),
    ] {
        storage.plugins().upsert(&record).await?;
    }

    storage
        .content_routing()
        .set_default("virtual", &primary)
        .await?;
    storage
        .content_routing()
        .update_participation(
            &primary,
            ContentParticipation {
                search_enabled: false,
                discover_enabled: true,
            },
        )
        .await?;

    let search = storage
        .content_routing()
        .list_candidates(ContentParticipationKind::Search, None, None)
        .await?;
    assert_eq!(
        search
            .iter()
            .map(|candidate| candidate.plugin_id.as_str())
            .collect::<Vec<_>>(),
        [
            "com.audiodown.catalog.primary",
            "com.audiodown.virtual.faster"
        ]
    );

    let discover = storage
        .content_routing()
        .list_candidates(ContentParticipationKind::Discover, Some("virtual"), None)
        .await?;
    assert_eq!(
        discover
            .iter()
            .map(|candidate| candidate.plugin_id.as_str())
            .collect::<Vec<_>>(),
        [
            "com.audiodown.virtual.primary",
            "com.audiodown.virtual.faster"
        ]
    );
    assert!(discover[0].is_default);
    assert!(!discover[1].is_default);

    let explicit = storage
        .content_routing()
        .list_candidates(ContentParticipationKind::Discover, None, Some(&faster))
        .await?;
    assert_eq!(explicit.len(), 1);
    assert_eq!(explicit[0].plugin_id, faster);
    Ok(())
}

#[tokio::test]
async fn replaces_one_default_per_platform_and_cleans_it_on_uninstall(
) -> Result<(), Box<dyn std::error::Error>> {
    let storage = migrated_storage().await?;
    let first = PluginId::parse("com.audiodown.virtual.first")?;
    let second = PluginId::parse("com.audiodown.virtual.second")?;
    storage
        .plugins()
        .upsert(&plugin_record(&first, PluginType::Content, "virtual", 100))
        .await?;
    storage
        .plugins()
        .upsert(&plugin_record(&second, PluginType::Content, "virtual", 50))
        .await?;

    storage
        .content_routing()
        .set_default("virtual", &first)
        .await?;
    assert_eq!(
        storage
            .content_routing()
            .default_for_platform("virtual")
            .await?,
        Some(first)
    );
    storage
        .content_routing()
        .set_default("virtual", &second)
        .await?;
    assert_eq!(
        storage
            .content_routing()
            .default_for_platform("virtual")
            .await?,
        Some(second.clone())
    );

    storage.plugins().delete(&second).await?;
    assert_eq!(
        storage
            .content_routing()
            .default_for_platform("virtual")
            .await?,
        None
    );
    Ok(())
}

#[tokio::test]
async fn rejects_non_content_wrong_platform_and_missing_defaults(
) -> Result<(), Box<dyn std::error::Error>> {
    let storage = migrated_storage().await?;
    let credential = PluginId::parse("com.audiodown.virtual.credential")?;
    let content = PluginId::parse("com.audiodown.catalog.content")?;
    let missing = PluginId::parse("com.audiodown.virtual.missing")?;
    storage
        .plugins()
        .upsert(&plugin_record(
            &credential,
            PluginType::Credential,
            "virtual",
            100,
        ))
        .await?;
    storage
        .plugins()
        .upsert(&plugin_record(
            &content,
            PluginType::Content,
            "catalog",
            100,
        ))
        .await?;

    assert!(matches!(
        storage
            .content_routing()
            .set_default("virtual", &credential)
            .await,
        Err(StorageError::InvalidData(_))
    ));
    assert!(matches!(
        storage
            .content_routing()
            .set_default("virtual", &content)
            .await,
        Err(StorageError::InvalidData(_))
    ));
    assert!(matches!(
        storage
            .content_routing()
            .set_default("virtual", &missing)
            .await,
        Err(StorageError::NotFound)
    ));
    assert!(matches!(
        storage
            .content_routing()
            .update_participation(
                &credential,
                ContentParticipation {
                    search_enabled: true,
                    discover_enabled: false,
                },
            )
            .await,
        Err(StorageError::InvalidData(_))
    ));
    Ok(())
}

async fn migrated_storage() -> Result<Storage, StorageError> {
    let storage = Storage::connect("sqlite::memory:").await?;
    storage.migrate().await?;
    Ok(storage)
}

fn plugin_record(
    plugin_id: &PluginId,
    plugin_type: PluginType,
    platform_id: &str,
    priority: i64,
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
            "capabilities": ["content.search", "content.discover"]
        }),
        manifest_hash: "manifest-hash".to_string(),
        source_hash: None,
        image_id: Some("sha256:virtual".to_string()),
        status: PluginStatus::Installed,
        run_mode: RunMode::OnDemand,
        priority,
        enabled: true,
        last_error: None,
        install_operation_id: None,
        last_used_at: None,
        installed_at: now,
        updated_at: now,
    }
}
