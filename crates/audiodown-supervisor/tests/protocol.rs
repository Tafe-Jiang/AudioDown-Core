use audiodown_domain::plugin::PluginId;
use audiodown_supervisor::protocol::{OperationStoreError, ProtocolOperationStore};
use audiodown_supervisor_protocol::{
    PluginInstallOperationState, SupervisorParams, SupervisorRequest,
};
use chrono::{Duration, TimeZone, Utc};
use uuid::Uuid;

#[test]
fn lifecycle_request_accepts_only_plugin_id() {
    let request: SupervisorRequest = serde_json::from_value(serde_json::json!({
        "id": "req-1",
        "token": "token",
        "timestamp": 1,
        "nonce": "nonce",
        "method": "plugin.start",
        "params": {
            "pluginId": "com.audiodown.virtual.content"
        }
    }))
    .unwrap();

    let Some(SupervisorParams::Plugin(params)) = request.validate_shape().unwrap() else {
        panic!("expected plugin params");
    };
    assert_eq!(params.plugin_id.as_str(), "com.audiodown.virtual.content");
}

#[test]
fn lifecycle_request_rejects_arbitrary_container_fields() {
    for field in ["image", "command", "mounts", "environment", "containerName"] {
        let mut params = serde_json::json!({
            "pluginId": "com.audiodown.virtual.content"
        });
        params[field] = serde_json::json!("caller-controlled");

        let request = serde_json::from_value::<SupervisorRequest>(serde_json::json!({
            "id": "req-1",
            "token": "token",
            "timestamp": 1,
            "nonce": "nonce",
            "method": "plugin.start",
            "params": params
        }));
        assert!(request.is_err(), "field {field} must be rejected");
    }
}

#[test]
fn operation_store_is_idempotent_and_scoped_to_the_installation() {
    let store = ProtocolOperationStore::default();
    let plugin_id = PluginId::parse("com.audiodown.virtual.content").unwrap();
    let other_plugin = PluginId::parse("com.audiodown.virtual.other").unwrap();
    let operation_id = Uuid::new_v4();
    let now = Utc.with_ymd_and_hms(2026, 7, 11, 9, 0, 0).unwrap();

    let first = store
        .begin("installation-a", plugin_id.clone(), operation_id, now)
        .unwrap();
    let repeated = store
        .begin("installation-a", plugin_id.clone(), operation_id, now)
        .unwrap();
    assert_eq!(first, repeated);
    assert_eq!(first.state, PluginInstallOperationState::Accepted);
    assert!(matches!(
        store.begin("installation-a", other_plugin, operation_id, now),
        Err(OperationStoreError::OperationIdMismatch)
    ));

    store
        .begin("installation-b", plugin_id.clone(), Uuid::new_v4(), now)
        .unwrap();
    for _ in 0..300 {
        store
            .begin("installation-a", plugin_id.clone(), Uuid::new_v4(), now)
            .unwrap();
    }
    let listed = store.list("installation-a").unwrap();
    assert_eq!(listed.operations.len(), 256);
    assert!(listed
        .operations
        .iter()
        .all(|operation| operation.plugin_id == plugin_id));
}

#[test]
fn terminal_operations_require_matching_ack_before_retention_cleanup() {
    let store = ProtocolOperationStore::default();
    let plugin_id = PluginId::parse("com.audiodown.virtual.content").unwrap();
    let wrong_plugin = PluginId::parse("com.audiodown.virtual.other").unwrap();
    let operation_id = Uuid::new_v4();
    let now = Utc.with_ymd_and_hms(2026, 7, 11, 9, 0, 0).unwrap();

    store
        .begin("installation-a", plugin_id.clone(), operation_id, now)
        .unwrap();
    assert!(matches!(
        store.acknowledge("installation-a", &plugin_id, operation_id, now),
        Err(OperationStoreError::NotTerminal)
    ));
    store
        .set_state(
            "installation-a",
            &plugin_id,
            operation_id,
            PluginInstallOperationState::Finalized,
            now,
        )
        .unwrap();
    store
        .cleanup_acknowledged_before(now + Duration::hours(1))
        .unwrap();
    assert!(store
        .get("installation-a", &plugin_id, operation_id)
        .is_ok());
    assert!(matches!(
        store.acknowledge("installation-a", &wrong_plugin, operation_id, now),
        Err(OperationStoreError::NotFound)
    ));

    let acknowledged = store
        .acknowledge("installation-a", &plugin_id, operation_id, now)
        .unwrap();
    assert!(acknowledged.acknowledged);
    assert!(store.list("installation-a").unwrap().operations.is_empty());
    store
        .cleanup_acknowledged_before(now + Duration::minutes(31))
        .unwrap();
    assert!(matches!(
        store.get("installation-a", &plugin_id, operation_id),
        Err(OperationStoreError::NotFound)
    ));
}
