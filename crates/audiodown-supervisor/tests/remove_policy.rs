use std::{collections::HashMap, fs};

use audiodown_domain::plugin::PluginId;
use audiodown_plugin_api::manifest::PluginManifest;
use audiodown_supervisor::{
    docker::managed_removal_plan, install_record::ValidatedInstall, policy::InstalledPlugin,
};
use tempfile::TempDir;

#[test]
fn derives_the_only_allowed_install_directory_and_expected_managed_assets() {
    let fixture = RemovalFixture::new();
    let plan =
        managed_removal_plan(fixture.temp.path(), "installation-test", &fixture.install).unwrap();

    assert_eq!(plan.plugin_id, fixture.install.installed.plugin_id);
    assert_eq!(plan.image_id, fixture.install.installed.image_id);
    assert_eq!(
        plan.install_directory,
        fixture
            .temp
            .path()
            .join("installed/com.audiodown.virtual.content")
    );
    assert_eq!(
        plan.expected_image_labels["io.audiodown.installation"],
        "installation-test"
    );
    assert_eq!(
        plan.expected_image_labels["io.audiodown.plugin-id"],
        "com.audiodown.virtual.content"
    );
}

#[test]
fn rejects_installation_identity_and_image_label_mismatches() {
    let fixture = RemovalFixture::new();
    assert!(
        managed_removal_plan(fixture.temp.path(), "other-installation", &fixture.install).is_err()
    );

    let mut mismatched = fixture.install.clone();
    mismatched.expected_image_labels.as_mut().unwrap().insert(
        "io.audiodown.plugin-id".to_string(),
        "com.audiodown.virtual.other".to_string(),
    );
    assert!(managed_removal_plan(fixture.temp.path(), "installation-test", &mismatched).is_err());
}

#[test]
fn rejects_symlinked_or_non_directory_install_roots() {
    let fixture = RemovalFixture::new();
    let plugin_dir = fixture
        .temp
        .path()
        .join("installed/com.audiodown.virtual.content");
    fs::remove_dir_all(&plugin_dir).unwrap();
    fs::write(&plugin_dir, b"not a directory").unwrap();
    assert!(
        managed_removal_plan(fixture.temp.path(), "installation-test", &fixture.install).is_err()
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;

        let linked = RemovalFixture::new();
        let plugin_dir = linked
            .temp
            .path()
            .join("installed/com.audiodown.virtual.content");
        fs::remove_dir_all(&plugin_dir).unwrap();
        let outside = linked.temp.path().join("outside");
        fs::create_dir_all(&outside).unwrap();
        symlink(outside, plugin_dir).unwrap();
        assert!(
            managed_removal_plan(linked.temp.path(), "installation-test", &linked.install).is_err()
        );
    }
}

struct RemovalFixture {
    temp: TempDir,
    install: ValidatedInstall,
}

impl RemovalFixture {
    fn new() -> Self {
        let temp = TempDir::new().unwrap();
        fs::create_dir_all(temp.path().join("installed/com.audiodown.virtual.content")).unwrap();
        let plugin_id = PluginId::parse("com.audiodown.virtual.content").unwrap();
        let image_id = format!("sha256:{}", "c".repeat(64));
        let expected_image_labels = HashMap::from([
            ("io.audiodown.managed".to_string(), "true".to_string()),
            (
                "io.audiodown.installation".to_string(),
                "installation-test".to_string(),
            ),
            ("io.audiodown.plugin-id".to_string(), plugin_id.to_string()),
            ("io.audiodown.commit-sha".to_string(), "0".repeat(40)),
            ("io.audiodown.source-hash".to_string(), "a".repeat(64)),
            ("io.audiodown.manifest-hash".to_string(), "b".repeat(64)),
            (
                "io.audiodown.base-image-digest".to_string(),
                format!("sha256:{}", "d".repeat(64)),
            ),
            ("io.audiodown.sdk-hash".to_string(), "e".repeat(64)),
        ]);
        let manifest: PluginManifest = serde_json::from_value(serde_json::json!({
            "schemaVersion": "1.0",
            "id": plugin_id,
            "name": "Virtual Content",
            "version": "1.0.0",
            "type": "content",
            "runtime": {"type": "nodejs", "version": "22", "entry": "src/index.js"},
            "compatibility": {"pluginApi": ">=1.0 <2.0", "core": ">=1.0 <2.0"},
            "platform": {"id": "virtual", "name": "Virtual"},
            "capabilities": ["content.search"],
            "network": {"allowedHosts": []}
        }))
        .unwrap();
        Self {
            temp,
            install: ValidatedInstall {
                installed: InstalledPlugin {
                    plugin_id,
                    image_id,
                    installation_id: "installation-test".to_string(),
                    runtime_path: "/plugin/src/index.js".to_string(),
                    memory_bytes: 128 * 1024 * 1024,
                    nano_cpus: 500_000_000,
                    pids_limit: 64,
                },
                manifest,
                expected_image_labels: Some(expected_image_labels),
            },
        }
    }
}
