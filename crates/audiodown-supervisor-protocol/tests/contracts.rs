use audiodown_domain::plugin::PluginId;
use audiodown_supervisor_protocol::{
    PluginBuildLog, PluginBuildLogStream, PluginInstallArtifact, PluginInstallOperation,
    PluginInstallOperationList, PluginInstallOperationState, PluginInstallOperationSummary,
    PluginInstallRequest, PluginRemoveResult, PluginRequest, ProtocolError, SupervisorMethod,
    SupervisorParams, SupervisorRequest, SupervisorResponse,
};
use uuid::Uuid;

#[test]
fn accepts_only_method_specific_parameter_shapes() {
    let install_methods = [
        "plugin.install.build",
        "plugin.install.status",
        "plugin.install.finalize",
        "plugin.install.abort",
        "plugin.install.ack",
    ];
    for method in install_methods {
        let request: SupervisorRequest = serde_json::from_value(serde_json::json!({
            "id": "req-1",
            "token": "token",
            "timestamp": 1,
            "nonce": "nonce",
            "method": method,
            "params": {
                "pluginId": "com.audiodown.virtual.content",
                "operationId": "75de0d58-03f9-4db7-8a27-69ac7ddce8de"
            }
        }))
        .unwrap();
        assert!(matches!(
            request.validate_shape(),
            Ok(Some(SupervisorParams::Install(_)))
        ));
    }

    for method in ["system.ping", "plugin.install.list"] {
        let request: SupervisorRequest = serde_json::from_value(serde_json::json!({
            "id": "req-1",
            "token": "token",
            "timestamp": 1,
            "nonce": "nonce",
            "method": method
        }))
        .unwrap();
        assert!(request.validate_shape().unwrap().is_none());
    }

    let remove: SupervisorRequest = serde_json::from_value(serde_json::json!({
        "id": "req-1",
        "token": "token",
        "timestamp": 1,
        "nonce": "nonce",
        "method": "plugin.remove",
        "params": {"pluginId": "com.audiodown.virtual.content"}
    }))
    .unwrap();
    assert!(matches!(
        remove.validate_shape(),
        Ok(Some(SupervisorParams::Plugin(_)))
    ));
}

#[test]
fn rejects_caller_controlled_install_and_remove_fields() {
    for field in [
        "image",
        "dockerfile",
        "command",
        "buildArgs",
        "network",
        "mounts",
        "environment",
        "sourcePath",
        "repositoryUrl",
        "allowScripts",
    ] {
        let mut params = serde_json::json!({
            "pluginId": "com.audiodown.virtual.content",
            "operationId": "75de0d58-03f9-4db7-8a27-69ac7ddce8de"
        });
        params[field] = serde_json::json!("caller-controlled");
        let request = serde_json::from_value::<SupervisorRequest>(serde_json::json!({
            "id": "req-1",
            "token": "token",
            "timestamp": 1,
            "nonce": "nonce",
            "method": "plugin.install.build",
            "params": params
        }));
        assert!(request.is_err(), "{field}");
    }

    assert!(
        serde_json::from_value::<PluginInstallRequest>(serde_json::json!({
            "pluginId": "com.audiodown.virtual.content",
            "operationId": Uuid::new_v4(),
            "image": "caller/image:latest"
        }))
        .is_err()
    );
    assert!(serde_json::from_value::<PluginRequest>(serde_json::json!({
        "pluginId": "com.audiodown.virtual.content",
        "mounts": ["/:/host"]
    }))
    .is_err());
}

#[test]
fn round_trips_complete_operation_contracts() {
    let plugin_id = PluginId::parse("com.audiodown.virtual.content").unwrap();
    let operation = PluginInstallOperation {
        operation_id: Uuid::new_v4(),
        plugin_id: plugin_id.clone(),
        state: PluginInstallOperationState::Built,
        artifact: Some(PluginInstallArtifact {
            image_id: "sha256:image".to_string(),
            repository_id: "example.plugins".to_string(),
            commit_sha: "0123456789abcdef0123456789abcdef01234567".to_string(),
            source_hash: "source-sha256".to_string(),
            manifest_hash: "manifest-sha256".to_string(),
        }),
        build_logs: vec![PluginBuildLog {
            sequence: 1,
            stream: PluginBuildLogStream::Stdout,
            message: "npm ci complete".to_string(),
        }],
        error_code: None,
        acknowledged: false,
    };
    let encoded = serde_json::to_value(&operation).unwrap();
    let decoded: PluginInstallOperation = serde_json::from_value(encoded).unwrap();
    assert_eq!(decoded, operation);

    let removed = PluginRemoveResult {
        plugin_id,
        removed_container: true,
        removed_image: true,
        removed_install_directory: true,
    };
    assert_eq!(
        serde_json::from_value::<PluginRemoveResult>(serde_json::to_value(&removed).unwrap())
            .unwrap(),
        removed
    );
}

#[test]
fn caps_operation_lists_and_marks_only_terminal_states_acknowledgeable() {
    let plugin_id = PluginId::parse("com.audiodown.virtual.content").unwrap();
    let operations = (0..300)
        .map(|_| PluginInstallOperationSummary {
            operation_id: Uuid::new_v4(),
            plugin_id: plugin_id.clone(),
            state: PluginInstallOperationState::Accepted,
            artifact: None,
            error_code: None,
            acknowledged: false,
        })
        .collect();
    let list = PluginInstallOperationList::new(operations);
    assert_eq!(list.operations.len(), 256);

    for state in [
        PluginInstallOperationState::Accepted,
        PluginInstallOperationState::Building,
        PluginInstallOperationState::Built,
    ] {
        assert!(!state.is_terminal());
    }
    for state in [
        PluginInstallOperationState::Finalized,
        PluginInstallOperationState::Failed,
        PluginInstallOperationState::Aborted,
    ] {
        assert!(state.is_terminal());
    }
}

#[test]
fn bounds_protocol_error_details_to_build_logs() {
    let valid = SupervisorResponse::failure_with_details(
        "req-1",
        "BUILD_FAILED",
        "Build failed",
        serde_json::json!({"buildLogs": []}),
    )
    .unwrap();
    assert!(valid.error.unwrap().details.is_some());

    assert!(matches!(
        SupervisorResponse::failure_with_details(
            "req-1",
            "BUILD_FAILED",
            "Build failed",
            serde_json::json!({"secret": "must-not-cross"})
        ),
        Err(ProtocolError::InvalidErrorDetails)
    ));
    assert!(matches!(
        SupervisorResponse::failure_with_details(
            "req-1",
            "BUILD_FAILED",
            "Build failed",
            serde_json::json!({"buildLogs": ["x".repeat(70 * 1024)]})
        ),
        Err(ProtocolError::ErrorDetailsTooLarge)
    ));
}

#[test]
fn rejects_unknown_top_level_fields_and_invalid_method_shapes() {
    assert!(
        serde_json::from_value::<SupervisorRequest>(serde_json::json!({
            "id": "req-1",
            "token": "token",
            "timestamp": 1,
            "nonce": "nonce",
            "method": "system.ping",
            "unexpected": true
        }))
        .is_err()
    );

    let missing: SupervisorRequest = serde_json::from_value(serde_json::json!({
        "id": "req-1",
        "token": "token",
        "timestamp": 1,
        "nonce": "nonce",
        "method": "plugin.install.build"
    }))
    .unwrap();
    assert!(matches!(
        missing.validate_shape(),
        Err(ProtocolError::MissingParams)
    ));

    let unexpected: SupervisorRequest = serde_json::from_value(serde_json::json!({
        "id": "req-1",
        "token": "token",
        "timestamp": 1,
        "nonce": "nonce",
        "method": "plugin.install.list",
        "params": {"pluginId": "com.audiodown.virtual.content"}
    }))
    .unwrap();
    assert!(matches!(
        unexpected.validate_shape(),
        Err(ProtocolError::UnexpectedParams)
    ));

    assert_eq!(
        SupervisorMethod::PluginInstallBuild.as_str(),
        "plugin.install.build"
    );
}
