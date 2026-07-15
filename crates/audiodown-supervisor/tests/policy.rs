use audiodown_domain::plugin::PluginId;
use audiodown_supervisor::docker::{
    discover_runtime_plugin_ids, network_is_healthy, network_is_owned_for_cleanup,
    plugin_container_config, proxy_token_publish_command, reconcile_cleanup_results,
    PROXY_TOKEN_SECRET_DIR,
};
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

#[test]
fn plugin_container_metadata_excludes_token_and_uses_tmpfs_bootstrap() {
    let token_canary = "docker-config-token-canary";
    let runtime = PluginRuntimePolicy::build(
        installed_plugin(PluginId::parse("com.audiodown.virtual.content").unwrap()),
        GatewayRuntimeConfig::default(),
        ProxyToken::new(token_canary).unwrap(),
    )
    .unwrap();

    let config = plugin_container_config(&runtime.plugin);
    let encoded = serde_json::to_string(&config).unwrap();
    assert!(!encoded.contains(token_canary));
    assert_eq!(config.entrypoint.as_ref().unwrap()[0], "/bin/sh");
    assert!(config
        .env
        .unwrap_or_default()
        .iter()
        .all(|entry| !entry.starts_with("AUDIODOWN_PROXY_TOKEN=")));
    assert!(config
        .host_config
        .unwrap()
        .tmpfs
        .unwrap()
        .contains_key(PROXY_TOKEN_SECRET_DIR));
}

#[test]
fn plugin_container_uses_fixed_inline_bootstrap_for_existing_images() {
    let token_canary = "inline-bootstrap-token-canary";
    let runtime = PluginRuntimePolicy::build(
        installed_plugin(PluginId::parse("com.audiodown.virtual.content").unwrap()),
        GatewayRuntimeConfig::default(),
        ProxyToken::new(token_canary).unwrap(),
    )
    .unwrap();

    let config = plugin_container_config(&runtime.plugin);
    let entrypoint = config.entrypoint.unwrap();
    assert_eq!(&entrypoint[..2], ["/bin/sh", "-c"]);
    assert_eq!(entrypoint[3], "audiodown-plugin-bootstrap");
    assert!(entrypoint[2].contains("/run/audiodown-secrets/proxy-token"));
    assert!(entrypoint[2].contains("rm -f \"$secret_file\""));
    assert!(entrypoint[2].contains("exec \"$@\""));
    assert!(!entrypoint.join(" ").contains(token_canary));
    assert!(!entrypoint
        .iter()
        .any(|argument| argument == "/usr/local/bin/audiodown-plugin-bootstrap"));
}

#[tokio::test]
async fn proxy_token_publish_is_atomic_under_delayed_chunked_input() {
    let directory = tempfile::tempdir().unwrap();
    let final_path = directory.path().join("proxy-token");
    let temporary_path = directory.path().join(".proxy-token.tmp");
    let token = b"chunked-runtime-proxy-token";
    let command = proxy_token_publish_command(token.len(), &temporary_path, &final_path);
    let mut child = tokio::process::Command::new(&command[0])
        .args(&command[1..])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();

    use tokio::io::AsyncWriteExt;
    input.write_all(&token[..4]).await.unwrap();
    input.flush().await.unwrap();
    for _ in 0..50 {
        if temporary_path.exists() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(temporary_path.exists());
    assert!(!final_path.exists());
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        std::fs::metadata(&temporary_path)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );

    input.write_all(&token[4..]).await.unwrap();
    drop(input);
    assert!(child.wait().await.unwrap().success());
    assert_eq!(std::fs::read(&final_path).unwrap(), token);
    assert!(!temporary_path.exists());
}

#[tokio::test]
async fn proxy_token_publish_rejects_short_input_and_cleans_temporary_file() {
    let directory = tempfile::tempdir().unwrap();
    let final_path = directory.path().join("proxy-token");
    let temporary_path = directory.path().join(".proxy-token.tmp");
    let command = proxy_token_publish_command(16, &temporary_path, &final_path);
    let mut child = tokio::process::Command::new(&command[0])
        .args(&command[1..])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();

    use tokio::io::AsyncWriteExt;
    input.write_all(b"short").await.unwrap();
    drop(input);
    assert!(!child.wait().await.unwrap().success());
    assert!(!final_path.exists());
    assert!(!temporary_path.exists());
}

#[test]
fn cleanup_ownership_is_independent_of_network_health() {
    let plugin_id = PluginId::parse("com.audiodown.virtual.content").unwrap();
    let labels = std::collections::HashMap::from([
        ("io.audiodown.managed".to_string(), "true".to_string()),
        (
            "io.audiodown.installation".to_string(),
            "installation-test".to_string(),
        ),
        ("io.audiodown.plugin-id".to_string(), plugin_id.to_string()),
        ("io.audiodown.resource".to_string(), "network".to_string()),
    ]);

    assert!(network_is_owned_for_cleanup(
        &labels,
        "installation-test",
        &plugin_id
    ));
    assert!(!network_is_healthy(false, true));
}

#[test]
fn startup_discovery_covers_all_runtime_resources_and_aggregates_failures() {
    let plugin = PluginId::parse("com.audiodown.virtual.content").unwrap();
    let gateway = PluginId::parse("com.audiodown.virtual.other").unwrap();
    let network = PluginId::parse("com.audiodown.virtual.network").unwrap();
    let labels = [
        runtime_labels(&plugin, "plugin"),
        runtime_labels(&gateway, "gateway"),
        runtime_labels(&network, "network"),
    ];

    let discovered = discover_runtime_plugin_ids("installation-test", labels.iter()).unwrap();
    assert_eq!(discovered.len(), 3);
    assert!(discovered.contains(&plugin));
    assert!(discovered.contains(&gateway));
    assert!(discovered.contains(&network));

    let error = reconcile_cleanup_results([Ok(()), Err(()), Ok(()), Err(())]).unwrap_err();
    assert!(error.to_string().contains('2'));
}

fn runtime_labels(
    plugin_id: &PluginId,
    resource: &str,
) -> std::collections::HashMap<String, String> {
    std::collections::HashMap::from([
        ("io.audiodown.managed".to_string(), "true".to_string()),
        (
            "io.audiodown.installation".to_string(),
            "installation-test".to_string(),
        ),
        ("io.audiodown.plugin-id".to_string(), plugin_id.to_string()),
        ("io.audiodown.resource".to_string(), resource.to_string()),
    ])
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
