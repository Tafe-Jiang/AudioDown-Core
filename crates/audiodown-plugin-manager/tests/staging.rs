use std::{fs, path::Path};

use audiodown_domain::plugin::PluginId;
use audiodown_plugin_api::manifest::{
    BuildSpec, CompatibilitySpec, LifecycleScriptPolicy, NetworkPolicy, PlatformSpec,
    PluginManifest, PluginType, RuntimeKind, RuntimeSpec,
};
use audiodown_plugin_manager::{
    archive::ExtractedSnapshot,
    github::GitHubRepositoryRef,
    staging::{LifecycleRiskGrant, SnapshotStore},
    validation::{ValidatedPlugin, ValidatedRepository},
};
use chrono::{Duration, TimeZone, Utc};
use serde_json::json;
use tempfile::TempDir;
use uuid::Uuid;

const COMMIT_SHA: &str = "0123456789abcdef0123456789abcdef01234567";
const MANIFEST_HASH: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const SOURCE_HASH: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

#[tokio::test]
async fn stages_validated_snapshot_and_prepares_exact_operation_metadata() {
    let fixture = SnapshotFixture::new(false);
    let preview = fixture.create().await;
    let snapshot_root = fixture
        .plugin_data()
        .join("staging")
        .join(preview.snapshot_id.to_string());

    assert_eq!(
        fs::read(snapshot_root.join("repository/plugins/virtual-content/src/index.js")).unwrap(),
        b"export default {};\n"
    );
    assert!(snapshot_root.join("snapshot.json").is_file());
    assert_mode(&snapshot_root, 0o700);
    assert_mode(&snapshot_root.join("repository"), 0o700);
    assert_mode(&snapshot_root.join("snapshot.json"), 0o600);

    let prepared = fixture
        .store
        .prepare_install(
            preview.snapshot_id,
            &PluginId::parse("com.audiodown.virtual.content").unwrap(),
            None,
        )
        .await
        .unwrap();
    let operation_path = fixture
        .plugin_data()
        .join("prepared")
        .join(format!("{}.json", prepared.operation_id));
    let operation: serde_json::Value =
        serde_json::from_slice(&fs::read(&operation_path).unwrap()).unwrap();

    assert_eq!(
        operation,
        json!({
            "schemaVersion": "1.0",
            "operationId": prepared.operation_id,
            "snapshotId": preview.snapshot_id,
            "pluginId": "com.audiodown.virtual.content",
            "repositoryId": "example.plugins",
            "sourceUrl": "https://github.com/example-owner/example-repository",
            "commitSha": COMMIT_SHA,
            "pluginPath": "plugins/virtual-content",
            "manifestHash": MANIFEST_HASH,
            "sourceHash": SOURCE_HASH,
            "allowLifecycleScripts": false,
            "riskGrantId": null
        })
    );
    assert_eq!(operation.as_object().unwrap().len(), 12);
    assert_mode(&operation_path, 0o600);
    assert_no_temporary_files(fixture.plugin_data());
}

#[tokio::test]
async fn mirrors_a_matching_lifecycle_script_grant() {
    let fixture = SnapshotFixture::new(true);
    let preview = fixture.create().await;
    let plugin_id = PluginId::parse("com.audiodown.virtual.content").unwrap();
    let granted_at = Utc.with_ymd_and_hms(2026, 7, 11, 8, 30, 0).unwrap();
    let grant = LifecycleRiskGrant {
        id: Uuid::new_v4(),
        repository_id: "example.plugins".to_string(),
        plugin_id: plugin_id.clone(),
        commit_sha: COMMIT_SHA.to_string(),
        risk_kind: "npm_lifecycle_scripts".to_string(),
        reason: "Generate a deterministic local file".to_string(),
        granted_at,
    };

    let prepared = fixture
        .store
        .prepare_install(preview.snapshot_id, &plugin_id, Some(&grant))
        .await
        .unwrap();
    let operation: serde_json::Value = serde_json::from_slice(
        &fs::read(
            fixture
                .plugin_data()
                .join("prepared")
                .join(format!("{}.json", prepared.operation_id)),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(operation["allowLifecycleScripts"], true);
    assert_eq!(operation["riskGrantId"], grant.id.to_string());

    let grant_path = fixture
        .plugin_data()
        .join("grants")
        .join(format!("{}.json", grant.id));
    let mirrored: serde_json::Value =
        serde_json::from_slice(&fs::read(&grant_path).unwrap()).unwrap();
    assert_eq!(
        mirrored,
        json!({
            "schemaVersion": "1.0",
            "grantId": grant.id,
            "repositoryId": "example.plugins",
            "pluginId": "com.audiodown.virtual.content",
            "commitSha": COMMIT_SHA,
            "riskKind": "npm_lifecycle_scripts",
            "reason": "Generate a deterministic local file",
            "grantedAt": granted_at
        })
    );
    assert_mode(&grant_path, 0o600);
}

#[tokio::test]
async fn rejects_tampered_snapshot_ids_and_missing_grants() {
    let fixture = SnapshotFixture::new(true);
    let preview = fixture.create().await;
    let snapshot_path = fixture
        .plugin_data()
        .join("staging")
        .join(preview.snapshot_id.to_string())
        .join("snapshot.json");
    let mut metadata: serde_json::Value =
        serde_json::from_slice(&fs::read(&snapshot_path).unwrap()).unwrap();
    metadata["snapshotId"] = json!(Uuid::new_v4());
    fs::write(&snapshot_path, serde_json::to_vec(&metadata).unwrap()).unwrap();

    assert!(fixture
        .store
        .prepare_install(
            preview.snapshot_id,
            &PluginId::parse("com.audiodown.virtual.content").unwrap(),
            None,
        )
        .await
        .is_err());
}

#[cfg(unix)]
#[tokio::test]
async fn cleans_expired_uuid_entries_without_traversing_symlinks() {
    use std::os::unix::fs::symlink;

    let fixture = SnapshotFixture::new(false);
    let now = Utc.with_ymd_and_hms(2026, 7, 11, 9, 0, 0).unwrap();
    let staging = fixture.plugin_data().join("staging");
    fs::create_dir_all(&staging).unwrap();

    let expired = staging.join(Uuid::new_v4().to_string());
    write_cleanup_metadata(&expired, now - Duration::minutes(31));
    let fresh = staging.join(Uuid::new_v4().to_string());
    write_cleanup_metadata(&fresh, now - Duration::minutes(29));
    let invalid = staging.join("not-a-uuid");
    write_cleanup_metadata(&invalid, now - Duration::hours(1));

    let outside = TempDir::new().unwrap();
    fs::write(outside.path().join("marker"), b"keep").unwrap();
    let linked = staging.join(Uuid::new_v4().to_string());
    symlink(outside.path(), &linked).unwrap();

    fixture.store.cleanup_expired(now).await.unwrap();

    assert!(!expired.exists());
    assert!(fresh.exists());
    assert!(invalid.exists());
    assert!(!linked.exists());
    assert_eq!(fs::read(outside.path().join("marker")).unwrap(), b"keep");
}

struct SnapshotFixture {
    temp: TempDir,
    store: SnapshotStore,
    requires_lifecycle_scripts: bool,
}

impl SnapshotFixture {
    fn new(requires_lifecycle_scripts: bool) -> Self {
        let temp = TempDir::new().unwrap();
        let store = SnapshotStore::new(temp.path().join("plugins"));
        Self {
            temp,
            store,
            requires_lifecycle_scripts,
        }
    }

    fn plugin_data(&self) -> &Path {
        self.store.plugin_data()
    }

    async fn create(&self) -> audiodown_plugin_manager::staging::RepositoryPreview {
        let extracted_root = self
            .temp
            .path()
            .join(format!("extracted-{}", Uuid::new_v4()));
        let plugin_root = extracted_root.join("plugins/virtual-content");
        fs::create_dir_all(plugin_root.join("src")).unwrap();
        fs::write(plugin_root.join("src/index.js"), b"export default {};\n").unwrap();

        self.store
            .create(
                &GitHubRepositoryRef::parse("https://github.com/example-owner/example-repository")
                    .unwrap(),
                COMMIT_SHA,
                ExtractedSnapshot {
                    repository_root: extracted_root,
                    file_count: 1,
                    extracted_bytes: 19,
                },
                validated_repository(self.requires_lifecycle_scripts),
            )
            .await
            .unwrap()
    }
}

fn validated_repository(requires_lifecycle_scripts: bool) -> ValidatedRepository {
    let reason =
        requires_lifecycle_scripts.then(|| "Generate a deterministic local file".to_string());
    ValidatedRepository {
        repository_id: "example.plugins".to_string(),
        repository_name: "Example Plugins".to_string(),
        plugins: vec![ValidatedPlugin {
            relative_path: "plugins/virtual-content".to_string(),
            manifest: PluginManifest {
                schema_version: "1.0".to_string(),
                id: PluginId::parse("com.audiodown.virtual.content").unwrap(),
                name: "Virtual Content".to_string(),
                version: "1.0.0".parse().unwrap(),
                plugin_type: PluginType::Content,
                runtime: RuntimeSpec {
                    kind: RuntimeKind::Nodejs,
                    version: "22".to_string(),
                    entry: "src/index.js".to_string(),
                },
                compatibility: CompatibilitySpec {
                    plugin_api: ">=1.0 <2.0".to_string(),
                    core: ">=1.0 <2.0".to_string(),
                },
                platform: PlatformSpec {
                    id: "virtual".to_string(),
                    name: "Virtual".to_string(),
                },
                capabilities: vec!["content.search".to_string()],
                network: NetworkPolicy {
                    allowed_hosts: Vec::new(),
                },
                build: BuildSpec {
                    npm_lifecycle_scripts: LifecycleScriptPolicy {
                        required: requires_lifecycle_scripts,
                        reason: reason.clone(),
                    },
                },
            },
            manifest_hash: MANIFEST_HASH.to_string(),
            source_hash: SOURCE_HASH.to_string(),
            entry_path: "src/index.js".to_string(),
            requires_lifecycle_scripts,
            lifecycle_script_reason: reason,
        }],
    }
}

fn write_cleanup_metadata(path: &Path, created_at: chrono::DateTime<Utc>) {
    fs::create_dir_all(path).unwrap();
    fs::write(
        path.join("snapshot.json"),
        serde_json::to_vec(&json!({"createdAt": created_at})).unwrap(),
    )
    .unwrap();
}

fn assert_no_temporary_files(root: &Path) {
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                pending.push(entry.path());
            } else {
                assert!(!entry.file_name().to_string_lossy().ends_with(".tmp"));
            }
        }
    }
}

#[cfg(unix)]
fn assert_mode(path: &Path, expected: u32) {
    use std::os::unix::fs::PermissionsExt;

    assert_eq!(
        fs::metadata(path).unwrap().permissions().mode() & 0o777,
        expected
    );
}

#[cfg(not(unix))]
fn assert_mode(_path: &Path, _expected: u32) {}
