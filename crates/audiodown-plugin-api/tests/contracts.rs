use audiodown_plugin_api::{
    manifest::{
        CredentialDeclarations, CredentialScopeDeclaration, CredentialTargetOrigin, PluginManifest,
        PluginType, RuntimeKind,
    },
    rpc::{JsonRpcRequest, PluginHello, PROTOCOL_VERSION},
};

#[test]
fn parses_minimal_node_content_manifest() {
    let manifest: PluginManifest = serde_json::from_str(
        r#"{
      "schemaVersion":"1.0",
      "id":"com.example.virtual.content",
      "name":"Virtual Content",
      "version":"1.0.0",
      "type":"content",
      "runtime":{"type":"nodejs","version":"22","entry":"src/index.js"},
      "compatibility":{"pluginApi":">=1.0 <2.0","core":">=1.0 <2.0"},
      "platform":{"id":"virtual","name":"Virtual"},
      "capabilities":["system.health"],
      "network":{"allowedHosts":[]}
    }"#,
    )
    .unwrap();

    assert_eq!(manifest.plugin_type, PluginType::Content);
    assert_eq!(manifest.runtime.kind, RuntimeKind::Nodejs);
    assert_eq!(manifest.id.as_str(), "com.example.virtual.content");
    assert!(manifest.credentials.is_empty());
}

#[test]
fn parses_strict_credential_scope_declarations() {
    let manifest: PluginManifest = serde_json::from_value(serde_json::json!({
        "schemaVersion": "1.0",
        "id": "com.example.virtual.credential",
        "name": "Virtual Credential",
        "version": "1.0.0",
        "type": "credential",
        "runtime": {"type": "nodejs", "version": "22", "entry": "src/index.js"},
        "compatibility": {"pluginApi": ">=1.0 <2.0", "core": ">=1.0 <2.0"},
        "platform": {"id": "virtual", "name": "Virtual"},
        "capabilities": ["credential.qr.start", "credential.status"],
        "network": {"allowedHosts": ["account.virtual.invalid"]},
        "credentials": {
            "providedScopes": [{
                "scope": "virtual.web",
                "targetOrigins": ["https://account.virtual.invalid"]
            }]
        }
    }))
    .unwrap();

    assert_eq!(
        manifest.credentials,
        CredentialDeclarations {
            provided_scopes: vec![CredentialScopeDeclaration {
                scope: audiodown_domain::credential::CredentialScope::parse("virtual.web").unwrap(),
                target_origins: vec![CredentialTargetOrigin::parse(
                    "https://account.virtual.invalid"
                )
                .unwrap()],
            }],
            required_scopes: Vec::new(),
            optional_scopes: Vec::new(),
        }
    );

    assert!(serde_json::from_value::<PluginManifest>(serde_json::json!({
        "schemaVersion": "1.0",
        "id": "com.example.virtual.credential",
        "name": "Virtual Credential",
        "version": "1.0.0",
        "type": "credential",
        "runtime": {"type": "nodejs", "version": "22", "entry": "src/index.js"},
        "compatibility": {"pluginApi": ">=1.0 <2.0", "core": ">=1.0 <2.0"},
        "platform": {"id": "virtual", "name": "Virtual"},
        "capabilities": ["credential.status"],
        "network": {"allowedHosts": ["account.virtual.invalid"]},
        "credentials": {
            "providedScopes": [{
                "scope": "virtual.web",
                "targetOrigins": ["https://account.virtual.invalid"]
            }],
            "script": "not allowed"
        }
    }))
    .is_err());
}

#[test]
fn serializes_json_rpc_hello_request() {
    let request = JsonRpcRequest::new(
        "req-1",
        "system.hello",
        PluginHello {
            protocol_version: PROTOCOL_VERSION.to_string(),
            core_version: "1.0.0-alpha.1".to_string(),
        },
    )
    .unwrap();

    let json = serde_json::to_value(request).unwrap();
    assert_eq!(json["jsonrpc"], "2.0");
    assert_eq!(json["method"], "system.hello");
    assert_eq!(json["params"]["protocolVersion"], "1.0");
}
