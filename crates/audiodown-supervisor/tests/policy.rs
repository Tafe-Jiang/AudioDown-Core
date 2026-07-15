use audiodown_domain::plugin::PluginId;
use audiodown_supervisor::policy::{
    GatewayRuntimeConfig, InstalledPlugin, PluginContainerPolicy, PluginRuntimePolicy,
    PROXY_BACKEND_SOCKET, PROXY_GATEWAY_ALIAS, PROXY_GATEWAY_URL,
};
use audiodown_supervisor_protocol::ProxyToken;

#[test]
fn generated_container_spec_enforces_security_invariants() {
    let installed = InstalledPlugin {
        plugin_id: PluginId::parse("com.audiodown.virtual.content").unwrap(),
        image_id: "audiodown/plugin-virtual:dev".to_string(),
        installation_id: "installation-test".to_string(),
        runtime_path: "/plugin/src/index.js".to_string(),
        memory_bytes: 128 * 1024 * 1024,
        nano_cpus: 500_000_000,
        pids_limit: 64,
    };

    let spec = PluginContainerPolicy::build(installed).unwrap();

    assert!(spec.read_only);
    assert_eq!(spec.memory_bytes, 128 * 1024 * 1024);
    assert_eq!(spec.nano_cpus, 500_000_000);
    assert_eq!(spec.pids_limit, 64);
    assert!(spec.cap_drop.contains(&"ALL".to_string()));
    assert!(!spec
        .mounts
        .iter()
        .any(|mount| mount.contains("docker.sock")));
    assert!(!spec.host_network);
    assert_eq!(spec.labels["io.audiodown.managed"], "true");
    assert!(!spec.privileged);
    assert!(spec.cap_add.is_empty());
    assert!(spec.public_ports.is_empty());
    assert!(spec
        .security_opt
        .contains(&"no-new-privileges:true".to_string()));
}

#[test]
fn derives_a_fixed_gateway_and_per_plugin_internal_network() {
    let plugin_id = PluginId::parse("com.audiodown.virtual.content").unwrap();
    let token_canary = "policy-proxy-token-canary";
    let runtime = PluginRuntimePolicy::build(
        installed_plugin(plugin_id.clone()),
        GatewayRuntimeConfig::new(
            "audiodown/plugin-gateway:1.0.0-alpha.1",
            "installation-proxy-volume",
        )
        .unwrap(),
        ProxyToken::new(token_canary).unwrap(),
    )
    .unwrap();

    assert!(runtime.network.internal);
    assert!(!runtime.network.attachable);
    assert!(runtime
        .network
        .name
        .starts_with("audiodown-plugin-network-"));
    assert_eq!(runtime.network.labels["io.audiodown.resource"], "network");
    assert_eq!(
        runtime.network.labels["io.audiodown.plugin-id"],
        plugin_id.as_str()
    );

    assert_eq!(runtime.plugin.network_name, runtime.network.name);
    assert_eq!(runtime.plugin.proxy_url, PROXY_GATEWAY_URL);
    assert_eq!(
        runtime
            .plugin
            .proxy_token
            .expose_secret(|value| value.to_string()),
        token_canary
    );
    assert!(runtime.plugin.mounts.is_empty());
    assert!(runtime.plugin.public_ports.is_empty());
    assert_eq!(runtime.plugin.dns, ["0.0.0.0"]);

    assert_eq!(
        runtime.gateway.image,
        "audiodown/plugin-gateway:1.0.0-alpha.1"
    );
    assert!(runtime
        .gateway
        .container_name
        .starts_with("audiodown-gateway-"));
    assert_eq!(runtime.gateway.network_name, runtime.network.name);
    assert_eq!(runtime.gateway.network_alias, PROXY_GATEWAY_ALIAS);
    assert_eq!(runtime.gateway.backend_socket, PROXY_BACKEND_SOCKET);
    assert_eq!(runtime.gateway.proxy_volume, "installation-proxy-volume");
    assert_eq!(
        runtime.gateway.mounts,
        ["installation-proxy-volume:/run/audiodown-proxy:ro"]
    );
    assert!(runtime.gateway.read_only);
    assert!(!runtime.gateway.privileged);
    assert!(runtime.gateway.cap_add.is_empty());
    assert_eq!(runtime.gateway.cap_drop, ["ALL"]);
    assert!(runtime
        .gateway
        .security_opt
        .contains(&"no-new-privileges:true".to_string()));
    assert!(runtime.gateway.public_ports.is_empty());
    assert!(runtime.gateway.memory_bytes > 0);
    assert!(runtime.gateway.nano_cpus > 0);
    assert!(runtime.gateway.pids_limit > 0);
    assert!(!format!("{runtime:?}").contains(token_canary));

    let other = PluginRuntimePolicy::build(
        installed_plugin(PluginId::parse("com.audiodown.virtual.other").unwrap()),
        GatewayRuntimeConfig::new(
            "audiodown/plugin-gateway:1.0.0-alpha.1",
            "installation-proxy-volume",
        )
        .unwrap(),
        ProxyToken::new("other-generation-token").unwrap(),
    )
    .unwrap();
    assert_ne!(runtime.plugin.container_name, other.plugin.container_name);
    assert_ne!(runtime.gateway.container_name, other.gateway.container_name);
    assert_ne!(runtime.network.name, other.network.name);
}

#[test]
fn rejects_invalid_deployment_owned_gateway_configuration() {
    for volume in ["", "/host/path", "volume:name", "white space"] {
        assert!(
            GatewayRuntimeConfig::new("audiodown/plugin-gateway:1.0.0-alpha.1", volume).is_err()
        );
    }
    for image in ["", "image with spaces", "\0"] {
        assert!(GatewayRuntimeConfig::new(image, "audiodown-proxy").is_err());
    }
}

fn installed_plugin(plugin_id: PluginId) -> InstalledPlugin {
    InstalledPlugin {
        plugin_id,
        image_id: "audiodown/plugin-virtual:dev".to_string(),
        installation_id: "installation-test".to_string(),
        runtime_path: "/plugin/src/index.js".to_string(),
        memory_bytes: 128 * 1024 * 1024,
        nano_cpus: 500_000_000,
        pids_limit: 64,
    }
}
