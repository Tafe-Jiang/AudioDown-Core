use audiodown_domain::{
    log::{LogLevel, StructuredLog},
    plugin::{PluginId, PluginStatus, RunMode},
};
use audiodown_plugin_api::manifest::PluginType;
use audiodown_storage::{LogFilter, PluginRecord, Storage};
use chrono::Utc;
use uuid::Uuid;

#[tokio::test]
async fn upgrades_the_phase_two_schema_with_content_routing(
) -> Result<(), Box<dyn std::error::Error>> {
    let pool = sqlx::SqlitePool::connect("sqlite::memory:").await?;
    sqlx::raw_sql(include_str!("../../../migrations/0001_initial.sql"))
        .execute(&pool)
        .await?;
    sqlx::raw_sql(include_str!(
        "../../../migrations/0002_plugin_installation.sql"
    ))
    .execute(&pool)
    .await?;
    sqlx::raw_sql(include_str!("../../../migrations/0003_content_routing.sql"))
        .execute(&pool)
        .await?;

    let columns = sqlx::query("PRAGMA table_info(plugins)")
        .fetch_all(&pool)
        .await?;
    let names = columns
        .iter()
        .map(|row| sqlx::Row::get::<String, _>(row, "name"))
        .collect::<Vec<_>>();
    assert!(names.contains(&"search_enabled".to_string()));
    assert!(names.contains(&"discover_enabled".to_string()));

    let table: String = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'platform_content_defaults'",
    )
    .fetch_one(&pool)
    .await?;
    assert_eq!(table, "platform_content_defaults");
    Ok(())
}

#[tokio::test]
async fn persists_plugin_state_and_structured_logs() -> Result<(), Box<dyn std::error::Error>> {
    let storage = Storage::connect("sqlite::memory:").await?;
    storage.migrate().await?;

    let plugin_id = PluginId::parse("com.audiodown.virtual.content")?;
    let now = Utc::now();
    let record = PluginRecord {
        plugin_id: plugin_id.clone(),
        plugin_type: PluginType::Content,
        platform_id: "virtual".to_string(),
        name: "Virtual Content".to_string(),
        version: "1.0.0".to_string(),
        protocol_version: "1.0".to_string(),
        source_kind: "fixture".to_string(),
        source_ref: "test-fixtures/plugins/virtual".to_string(),
        commit_sha: None,
        repository_id: None,
        manifest_json: serde_json::json!({"id": plugin_id.as_str()}),
        manifest_hash: "fixture-hash".to_string(),
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
    };

    storage.plugins().upsert(&record).await?;
    storage
        .plugins()
        .set_status(&plugin_id, PluginStatus::Healthy)
        .await?;

    let plugin = storage.plugins().get(&plugin_id).await?.unwrap();
    assert_eq!(plugin.status, PluginStatus::Healthy);

    let log = StructuredLog {
        id: Uuid::new_v4(),
        timestamp: Utc::now(),
        level: LogLevel::Info,
        component: "virtual-plugin".to_string(),
        message: "virtual plugin ready".to_string(),
        plugin_id: Some(plugin_id.as_str().to_string()),
        plugin_version: Some("1.0.0".to_string()),
        platform_id: Some("virtual".to_string()),
        request_id: None,
        task_id: None,
        container_id: None,
        error_code: None,
        context: serde_json::json!({"healthy": true}),
    };
    storage.logs().append(&log).await?;

    let logs = storage
        .logs()
        .list(LogFilter {
            plugin_id: Some(plugin_id),
            limit: 50,
        })
        .await?;
    assert_eq!(logs.len(), 1);
    assert_eq!(logs[0].message, "virtual plugin ready");

    Ok(())
}
