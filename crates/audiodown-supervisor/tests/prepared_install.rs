use std::{
    fs,
    path::{Path, PathBuf},
};

use audiodown_domain::plugin::PluginId;
use audiodown_supervisor::prepared_install::validate_prepared_install;
use serde_json::json;
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use uuid::Uuid;

const OPERATION_ID: &str = "75de0d58-03f9-4db7-8a27-69ac7ddce8de";
const SNAPSHOT_ID: &str = "65ddab42-9e2f-4de1-a159-705bf9d055e9";
const PLUGIN_ID: &str = "com.audiodown.virtual.content";
const COMMIT_SHA: &str = "0123456789abcdef0123456789abcdef01234567";

#[tokio::test]
async fn derives_and_revalidates_a_prepared_install() {
    let fixture = PreparedFixture::new();
    let validated = validate_prepared_install(
        fixture.plugin_data(),
        &PluginId::parse(PLUGIN_ID).unwrap(),
        Uuid::parse_str(OPERATION_ID).unwrap(),
    )
    .await
    .unwrap();

    assert_eq!(validated.operation_id.to_string(), OPERATION_ID);
    assert_eq!(validated.snapshot_id.to_string(), SNAPSHOT_ID);
    assert_eq!(validated.plugin_id.as_str(), PLUGIN_ID);
    assert_eq!(validated.repository_id, "example.plugins");
    assert_eq!(validated.commit_sha, COMMIT_SHA);
    assert_eq!(validated.plugin_path, "plugins/virtual-content");
    assert_eq!(
        validated.plugin_root,
        fixture
            .plugin_data()
            .join("staging")
            .join(SNAPSHOT_ID)
            .join("repository/plugins/virtual-content")
    );
    assert!(!validated.allow_lifecycle_scripts);
    assert!(validated.risk_grant_id.is_none());
}

#[tokio::test]
async fn rejects_request_identity_and_schema_mismatches() {
    let fixture = PreparedFixture::new();
    assert!(validate_prepared_install(
        fixture.plugin_data(),
        &PluginId::parse("com.audiodown.virtual.other").unwrap(),
        Uuid::parse_str(OPERATION_ID).unwrap(),
    )
    .await
    .is_err());
    assert!(validate_prepared_install(
        fixture.plugin_data(),
        &PluginId::parse(PLUGIN_ID).unwrap(),
        Uuid::new_v4(),
    )
    .await
    .is_err());

    fixture.mutate_prepared(|value| value["schemaVersion"] = json!("2.0"));
    assert!(fixture.validate().await.is_err());
}

#[tokio::test]
async fn rejects_missing_snapshot_traversal_and_symlink_paths() {
    let fixture = PreparedFixture::new();
    fs::remove_dir_all(fixture.plugin_data().join("staging").join(SNAPSHOT_ID)).unwrap();
    assert!(fixture.validate().await.is_err());

    let traversal = PreparedFixture::new();
    traversal.mutate_prepared(|value| value["pluginPath"] = json!("../outside"));
    assert!(traversal.validate().await.is_err());

    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;

        let linked = PreparedFixture::new();
        let plugin_root = linked.plugin_root();
        fs::remove_dir_all(&plugin_root).unwrap();
        let outside = linked.temp.path().join("outside");
        fs::create_dir_all(&outside).unwrap();
        symlink(outside, plugin_root).unwrap();
        assert!(linked.validate().await.is_err());
    }
}

#[tokio::test]
async fn rejects_manifest_source_commit_and_repository_mismatches() {
    let manifest = PreparedFixture::new();
    fs::write(manifest.plugin_root().join("audiodown-plugin.json"), b"{}").unwrap();
    assert!(manifest.validate().await.is_err());

    let source = PreparedFixture::new();
    fs::write(source.plugin_root().join("src/extra.js"), b"changed").unwrap();
    assert!(source.validate().await.is_err());

    let commit = PreparedFixture::new();
    commit.mutate_snapshot(|value| value["commitSha"] = json!("f".repeat(40)));
    assert!(commit.validate().await.is_err());

    let repository = PreparedFixture::new();
    repository.mutate_snapshot(|value| value["repositoryId"] = json!("other.repository"));
    assert!(repository.validate().await.is_err());
}

#[tokio::test]
async fn requires_a_matching_mirrored_lifecycle_grant() {
    let missing_id = PreparedFixture::new();
    missing_id.mutate_prepared(|value| value["allowLifecycleScripts"] = json!(true));
    assert!(missing_id.validate().await.is_err());

    let missing_mirror = PreparedFixture::new();
    missing_mirror.enable_lifecycle_grant(false);
    assert!(missing_mirror.validate().await.is_err());

    let valid = PreparedFixture::new();
    valid.enable_lifecycle_grant(true);
    let prepared = valid.validate().await.unwrap();
    assert!(prepared.allow_lifecycle_scripts);
    assert_eq!(
        prepared.risk_grant_id,
        Some(Uuid::parse_str("85df1e67-a533-42dc-81e1-7b18687840fe").unwrap())
    );

    valid.mutate_grant(|value| value["commitSha"] = json!("f".repeat(40)));
    assert!(valid.validate().await.is_err());
}

#[tokio::test]
async fn rejects_non_node22_runtime_and_non_registry_lockfile_urls() {
    let runtime = PreparedFixture::new();
    runtime.mutate_manifest(|value| value["runtime"]["version"] = json!("20"));
    runtime.refresh_hashes();
    assert!(runtime.validate().await.is_err());

    let dependency = PreparedFixture::new();
    dependency.mutate_lockfile(|value| {
        value["packages"]["node_modules/example"] = json!({
            "version": "1.0.0",
            "resolved": "https://packages.invalid/example.tgz",
            "integrity": "sha512-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=="
        });
    });
    dependency.refresh_hashes();
    assert!(dependency.validate().await.is_err());
}

struct PreparedFixture {
    temp: TempDir,
}

impl PreparedFixture {
    fn new() -> Self {
        let temp = TempDir::new().unwrap();
        let fixture = Self { temp };
        fs::create_dir_all(fixture.plugin_root().join("src")).unwrap();
        fs::create_dir_all(fixture.plugin_data().join("prepared")).unwrap();
        fs::create_dir_all(fixture.plugin_data().join("grants")).unwrap();
        fs::write(
            fixture.plugin_root().join("src/index.js"),
            b"export default {};\n",
        )
        .unwrap();
        fs::write(
            fixture.plugin_root().join("audiodown-plugin.json"),
            serde_json::to_vec(&manifest()).unwrap(),
        )
        .unwrap();
        fs::write(
            fixture.plugin_root().join("package.json"),
            br#"{"name":"virtual-content","version":"1.0.0"}"#,
        )
        .unwrap();
        fs::write(
            fixture.plugin_root().join("package-lock.json"),
            serde_json::to_vec(&lockfile()).unwrap(),
        )
        .unwrap();
        fixture.write_metadata();
        fixture
    }

    fn plugin_data(&self) -> &Path {
        self.temp.path()
    }

    fn plugin_root(&self) -> PathBuf {
        self.plugin_data()
            .join("staging")
            .join(SNAPSHOT_ID)
            .join("repository/plugins/virtual-content")
    }

    fn prepared_path(&self) -> PathBuf {
        self.plugin_data()
            .join("prepared")
            .join(format!("{OPERATION_ID}.json"))
    }

    fn snapshot_path(&self) -> PathBuf {
        self.plugin_data()
            .join("staging")
            .join(SNAPSHOT_ID)
            .join("snapshot.json")
    }

    fn grant_path(&self) -> PathBuf {
        self.plugin_data()
            .join("grants/85df1e67-a533-42dc-81e1-7b18687840fe.json")
    }

    async fn validate(
        &self,
    ) -> Result<
        audiodown_supervisor::prepared_install::ValidatedPreparedInstall,
        audiodown_supervisor::prepared_install::PreparedInstallError,
    > {
        validate_prepared_install(
            self.plugin_data(),
            &PluginId::parse(PLUGIN_ID).unwrap(),
            Uuid::parse_str(OPERATION_ID).unwrap(),
        )
        .await
    }

    fn mutate_prepared(&self, mutate: impl FnOnce(&mut serde_json::Value)) {
        mutate_json(self.prepared_path(), mutate);
    }

    fn mutate_snapshot(&self, mutate: impl FnOnce(&mut serde_json::Value)) {
        mutate_json(self.snapshot_path(), mutate);
    }

    fn mutate_manifest(&self, mutate: impl FnOnce(&mut serde_json::Value)) {
        mutate_json(self.plugin_root().join("audiodown-plugin.json"), mutate);
    }

    fn mutate_lockfile(&self, mutate: impl FnOnce(&mut serde_json::Value)) {
        mutate_json(self.plugin_root().join("package-lock.json"), mutate);
    }

    fn mutate_package(&self, mutate: impl FnOnce(&mut serde_json::Value)) {
        mutate_json(self.plugin_root().join("package.json"), mutate);
    }

    fn mutate_grant(&self, mutate: impl FnOnce(&mut serde_json::Value)) {
        mutate_json(self.grant_path(), mutate);
    }

    fn enable_lifecycle_grant(&self, write_mirror: bool) {
        let grant_id = "85df1e67-a533-42dc-81e1-7b18687840fe";
        self.mutate_manifest(|value| {
            value["build"] = json!({
                "npmLifecycleScripts": {
                    "required": true,
                    "reason": "Generate a deterministic local file"
                }
            });
        });
        self.mutate_package(|value| {
            value["scripts"] = json!({"install": "node src/index.js"});
        });
        self.refresh_hashes();
        self.mutate_prepared(|value| {
            value["allowLifecycleScripts"] = json!(true);
            value["riskGrantId"] = json!(grant_id);
        });
        self.mutate_snapshot(|value| {
            value["plugins"][0]["requiresLifecycleScripts"] = json!(true);
            value["plugins"][0]["lifecycleScriptReason"] =
                json!("Generate a deterministic local file");
        });
        if write_mirror {
            fs::write(
                self.grant_path(),
                serde_json::to_vec(&json!({
                    "schemaVersion": "1.0",
                    "grantId": grant_id,
                    "repositoryId": "example.plugins",
                    "pluginId": PLUGIN_ID,
                    "commitSha": COMMIT_SHA,
                    "riskKind": "npm_lifecycle_scripts",
                    "reason": "Generate a deterministic local file",
                    "grantedAt": "2026-07-11T08:30:00Z"
                }))
                .unwrap(),
            )
            .unwrap();
        }
    }

    fn refresh_hashes(&self) {
        self.write_metadata();
    }

    fn write_metadata(&self) {
        let manifest_hash = sha256_file(&self.plugin_root().join("audiodown-plugin.json"));
        let source_hash = source_hash(&self.plugin_root());
        fs::write(
            self.snapshot_path(),
            serde_json::to_vec(&json!({
                "schemaVersion": "1.0",
                "snapshotId": SNAPSHOT_ID,
                "repositoryId": "example.plugins",
                "repositoryName": "Example Plugins",
                "sourceUrl": "https://github.com/example-owner/example-repository",
                "commitSha": COMMIT_SHA,
                "createdAt": "2026-07-11T08:00:00Z",
                "fileCount": 4,
                "extractedBytes": 512,
                "plugins": [{
                    "pluginId": PLUGIN_ID,
                    "name": "Virtual Content",
                    "version": "1.0.0",
                    "pluginType": "content",
                    "pluginPath": "plugins/virtual-content",
                    "manifestHash": manifest_hash,
                    "sourceHash": source_hash,
                    "requiresLifecycleScripts": false,
                    "lifecycleScriptReason": null
                }]
            }))
            .unwrap(),
        )
        .unwrap();
        fs::write(
            self.prepared_path(),
            serde_json::to_vec(&json!({
                "schemaVersion": "1.0",
                "operationId": OPERATION_ID,
                "snapshotId": SNAPSHOT_ID,
                "pluginId": PLUGIN_ID,
                "repositoryId": "example.plugins",
                "sourceUrl": "https://github.com/example-owner/example-repository",
                "commitSha": COMMIT_SHA,
                "pluginPath": "plugins/virtual-content",
                "manifestHash": manifest_hash,
                "sourceHash": source_hash,
                "allowLifecycleScripts": false,
                "riskGrantId": null
            }))
            .unwrap(),
        )
        .unwrap();
    }
}

fn manifest() -> serde_json::Value {
    json!({
        "schemaVersion": "1.0",
        "id": PLUGIN_ID,
        "name": "Virtual Content",
        "version": "1.0.0",
        "type": "content",
        "runtime": {"type": "nodejs", "version": "22", "entry": "src/index.js"},
        "compatibility": {"pluginApi": ">=1.0 <2.0", "core": ">=1.0 <2.0"},
        "platform": {"id": "virtual", "name": "Virtual"},
        "capabilities": ["content.search"],
        "network": {"allowedHosts": []}
    })
}

fn lockfile() -> serde_json::Value {
    json!({
        "name": "virtual-content",
        "version": "1.0.0",
        "lockfileVersion": 3,
        "packages": {
            "": {"name": "virtual-content", "version": "1.0.0"}
        }
    })
}

fn mutate_json(path: PathBuf, mutate: impl FnOnce(&mut serde_json::Value)) {
    let mut value: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    mutate(&mut value);
    fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
}

fn sha256_file(path: &Path) -> String {
    format!("{:x}", Sha256::digest(fs::read(path).unwrap()))
}

fn source_hash(root: &Path) -> String {
    let mut files = Vec::new();
    collect_files(root, root, &mut files);
    files.sort_by(|left, right| left.0.cmp(&right.0));
    let mut hasher = Sha256::new();
    for (relative, path) in files {
        let content = fs::read(path).unwrap();
        hasher.update((relative.len() as u64).to_be_bytes());
        hasher.update(relative.as_bytes());
        hasher.update((content.len() as u64).to_be_bytes());
        hasher.update(content);
    }
    format!("{:x}", hasher.finalize())
}

fn collect_files(root: &Path, directory: &Path, files: &mut Vec<(String, PathBuf)>) {
    for entry in fs::read_dir(directory).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() {
            collect_files(root, &entry.path(), files);
        } else {
            files.push((
                entry
                    .path()
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace(std::path::MAIN_SEPARATOR, "/"),
                entry.path(),
            ));
        }
    }
}
