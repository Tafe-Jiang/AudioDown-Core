use audiodown_plugin_api::{manifest::PluginManifest, repository::RepositoryIndex};

#[test]
fn parses_repository_index_and_declared_build_risk() {
    let index: RepositoryIndex = serde_json::from_value(serde_json::json!({
        "schemaVersion": "1.0",
        "repository": {
            "id": "example.plugins",
            "name": "Example Plugins"
        },
        "plugins": [
            {"path": "plugins/virtual-content"}
        ]
    }))
    .unwrap();
    assert_eq!(index.repository.id, "example.plugins");
    assert_eq!(index.plugins[0].path, "plugins/virtual-content");

    let manifest: PluginManifest = serde_json::from_value(serde_json::json!({
        "schemaVersion": "1.0",
        "id": "com.audiodown.virtual.content",
        "name": "Virtual Content",
        "version": "1.0.0",
        "type": "content",
        "runtime": {"type": "nodejs", "version": "22", "entry": "src/index.js"},
        "compatibility": {"pluginApi": ">=1.0 <2.0", "core": ">=1.0 <2.0"},
        "platform": {"id": "virtual", "name": "Virtual"},
        "capabilities": [],
        "network": {"allowedHosts": []},
        "build": {
            "npmLifecycleScripts": {
                "required": true,
                "reason": "Generate a deterministic local file"
            }
        }
    }))
    .unwrap();
    assert!(manifest.build.npm_lifecycle_scripts.required);
}

#[test]
fn manifest_defaults_to_no_lifecycle_scripts() {
    let manifest: PluginManifest = serde_json::from_str(include_str!(
        "../../../test-fixtures/plugins/virtual/audiodown-plugin.json"
    ))
    .unwrap();
    assert!(!manifest.build.npm_lifecycle_scripts.required);
    assert!(manifest.credentials.is_empty());
}

#[test]
fn parses_content_requested_scopes_and_exact_origins() {
    let manifest: PluginManifest = serde_json::from_value(serde_json::json!({
        "schemaVersion": "1.0",
        "id": "com.audiodown.virtual.content",
        "name": "Virtual Content",
        "version": "1.0.0",
        "type": "content",
        "runtime": {"type": "nodejs", "version": "22", "entry": "src/index.js"},
        "compatibility": {"pluginApi": ">=1.0 <2.0", "core": ">=1.0 <2.0"},
        "platform": {"id": "virtual", "name": "Virtual"},
        "capabilities": ["content.search"],
        "network": {
            "allowedHosts": [
                "account.virtual.invalid",
                "media.virtual.invalid"
            ]
        },
        "credentials": {
            "requiredScopes": [{
                "scope": "virtual.web",
                "targetOrigins": ["https://account.virtual.invalid"]
            }],
            "optionalScopes": [{
                "scope": "virtual.media",
                "targetOrigins": ["https://media.virtual.invalid:8443"]
            }]
        }
    }))
    .unwrap();

    assert_eq!(manifest.credentials.required_scopes.len(), 1);
    assert_eq!(manifest.credentials.optional_scopes.len(), 1);
    assert_eq!(
        manifest.credentials.optional_scopes[0].target_origins[0].as_str(),
        "https://media.virtual.invalid:8443"
    );
}
