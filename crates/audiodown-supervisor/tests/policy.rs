use audiodown_domain::plugin::PluginId;
use audiodown_supervisor::policy::{InstalledPlugin, PluginContainerPolicy};

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
