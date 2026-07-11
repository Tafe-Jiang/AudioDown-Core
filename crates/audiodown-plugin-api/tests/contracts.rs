use audiodown_plugin_api::{
    manifest::{PluginManifest, PluginType, RuntimeKind},
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
