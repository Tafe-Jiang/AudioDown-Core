use audiodown_supervisor::protocol::{PluginRequest, SupervisorRequest};

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

    assert_eq!(
        request.params.unwrap().plugin_id.as_str(),
        "com.audiodown.virtual.content"
    );
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

    let direct = serde_json::from_value::<PluginRequest>(serde_json::json!({
        "pluginId": "com.audiodown.virtual.content",
        "image": "caller/image:latest"
    }));
    assert!(direct.is_err());
}
