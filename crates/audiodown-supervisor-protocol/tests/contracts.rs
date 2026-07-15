use audiodown_domain::plugin::PluginId;
use audiodown_plugin_api::{
    content::ContentMethod,
    rpc::{JsonRpcError, JsonRpcResponse},
};
use audiodown_supervisor_protocol::{
    PluginBuildLog, PluginBuildLogStream, PluginInstallArtifact, PluginInstallOperation,
    PluginInstallOperationList, PluginInstallOperationState, PluginInstallOperationSummary,
    PluginInstallRequest, PluginRemoveResult, PluginRequest, PluginRpcRequest, PluginRpcResult,
    PluginStartRequest, ProtocolError, ProxyToken, SupervisorMethod, SupervisorParams,
    SupervisorRequest, SupervisorResponse,
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

    let rpc: SupervisorRequest = serde_json::from_value(serde_json::json!({
        "id": "req-1",
        "token": "token",
        "timestamp": 1,
        "nonce": "nonce",
        "method": "plugin.rpc",
        "params": {
            "pluginId": "com.audiodown.virtual.content",
            "method": "content.search",
            "params": {
                "query": "virtual",
                "limit": 20
            }
        }
    }))
    .unwrap();
    assert!(matches!(
        rpc.validate_shape(),
        Ok(Some(SupervisorParams::Rpc(_)))
    ));
}

#[test]
fn plugin_start_requires_only_a_plugin_id_and_redacted_proxy_token() {
    let token_canary = "proxy-token-contract-canary";
    let request: SupervisorRequest = serde_json::from_value(serde_json::json!({
        "id": "req-start",
        "token": "control-token",
        "timestamp": 1,
        "nonce": "nonce",
        "method": "plugin.start",
        "params": {
            "pluginId": "com.audiodown.virtual.content",
            "proxyToken": token_canary
        }
    }))
    .unwrap();

    let Some(SupervisorParams::Start(params)) = request.validate_shape().unwrap() else {
        panic!("expected trusted start params");
    };
    assert_eq!(params.plugin_id.as_str(), "com.audiodown.virtual.content");
    assert_eq!(
        params.proxy_token.expose_secret(|value| value.to_string()),
        token_canary
    );
    assert!(!format!("{params:?}").contains(token_canary));
    assert!(!format!("{:?}", params.proxy_token).contains(token_canary));

    let encoded = serde_json::to_value(PluginStartRequest {
        plugin_id: params.plugin_id.clone(),
        proxy_token: ProxyToken::new(token_canary).unwrap(),
    })
    .unwrap();
    assert_eq!(
        encoded,
        serde_json::json!({
            "pluginId": "com.audiodown.virtual.content",
            "proxyToken": token_canary
        })
    );
}

#[test]
fn plugin_start_rejects_missing_invalid_and_caller_controlled_runtime_fields() {
    for invalid in ["", "contains\0nul"] {
        assert!(ProxyToken::new(invalid).is_err());
    }
    assert!(ProxyToken::new(&"x".repeat(4097)).is_err());

    let missing: SupervisorRequest = serde_json::from_value(serde_json::json!({
        "id": "req-start",
        "token": "control-token",
        "timestamp": 1,
        "nonce": "nonce",
        "method": "plugin.start",
        "params": {"pluginId": "com.audiodown.virtual.content"}
    }))
    .unwrap();
    assert!(matches!(
        missing.validate_shape(),
        Err(ProtocolError::InvalidParams)
    ));

    for field in [
        "image",
        "command",
        "socketPath",
        "volumeName",
        "mounts",
        "network",
        "networkAlias",
        "environment",
    ] {
        let mut params = serde_json::json!({
            "pluginId": "com.audiodown.virtual.content",
            "proxyToken": "trusted-token"
        });
        params[field] = serde_json::json!("caller-controlled");
        let decoded = serde_json::from_value::<SupervisorRequest>(serde_json::json!({
            "id": "req-start",
            "token": "control-token",
            "timestamp": 1,
            "nonce": "nonce",
            "method": "plugin.start",
            "params": params
        }));
        assert!(decoded.is_err(), "field {field} must be rejected");
    }

    let stop_with_token: SupervisorRequest = serde_json::from_value(serde_json::json!({
        "id": "req-stop",
        "token": "control-token",
        "timestamp": 1,
        "nonce": "nonce",
        "method": "plugin.stop",
        "params": {
            "pluginId": "com.audiodown.virtual.content",
            "proxyToken": "not-allowed-on-stop"
        }
    }))
    .unwrap();
    assert!(matches!(
        stop_with_token.validate_shape(),
        Err(ProtocolError::InvalidParams)
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
fn plugin_rpc_accepts_only_typed_bounded_content_calls() {
    let plugin_id = PluginId::parse("com.audiodown.virtual.content").unwrap();
    let request = PluginRpcRequest {
        plugin_id: plugin_id.clone(),
        method: ContentMethod::Search,
        params: serde_json::json!({"query": "virtual", "limit": 20}),
    };
    request.validate().unwrap();

    for field in [
        "timeout",
        "command",
        "socketPath",
        "containerId",
        "mounts",
        "environment",
    ] {
        let mut params = serde_json::json!({
            "pluginId": plugin_id,
            "method": "content.search",
            "params": {"query": "virtual", "limit": 20}
        });
        params[field] = serde_json::json!("caller-controlled");
        assert!(
            serde_json::from_value::<PluginRpcRequest>(params).is_err(),
            "{field}"
        );
    }

    assert!(
        serde_json::from_value::<PluginRpcRequest>(serde_json::json!({
            "pluginId": plugin_id,
            "method": "content.download.plan",
            "params": {}
        }))
        .is_err()
    );

    let invalid: SupervisorRequest = serde_json::from_value(serde_json::json!({
        "id": "req-1",
        "token": "token",
        "timestamp": 1,
        "nonce": "nonce",
        "method": "plugin.rpc",
        "params": {
            "pluginId": plugin_id,
            "method": "content.search",
            "params": {"query": "", "limit": 20}
        }
    }))
    .unwrap();
    assert!(matches!(
        invalid.validate_shape(),
        Err(ProtocolError::InvalidRpcParams)
    ));

    let oversized = PluginRpcRequest {
        plugin_id,
        method: ContentMethod::Search,
        params: serde_json::json!({
            "query": "virtual",
            "limit": 20,
            "padding": "x".repeat(1024 * 1024)
        }),
    };
    assert!(matches!(
        oversized.validate(),
        Err(ProtocolError::RpcParamsTooLarge)
    ));
}

#[test]
fn plugin_rpc_result_bounds_the_json_rpc_response() {
    let response = JsonRpcResponse {
        jsonrpc: "2.0".to_string(),
        id: "plugin-request".to_string(),
        result: Some(serde_json::json!({"items": []})),
        error: None,
    };
    let result = PluginRpcResult::new(response).unwrap();
    assert_eq!(result.response.id, "plugin-request");

    let oversized = JsonRpcResponse {
        jsonrpc: "2.0".to_string(),
        id: "plugin-request".to_string(),
        result: None,
        error: Some(JsonRpcError {
            code: -32000,
            message: "Plugin call failed".to_string(),
            data: Some(serde_json::json!({"summary": "x".repeat(1024 * 1024)})),
        }),
    };
    assert!(matches!(
        PluginRpcResult::new(oversized),
        Err(ProtocolError::RpcResponseTooLarge)
    ));
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
    assert_eq!(SupervisorMethod::PluginRpc.as_str(), "plugin.rpc");
}
