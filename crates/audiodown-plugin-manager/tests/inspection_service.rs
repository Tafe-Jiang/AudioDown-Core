use std::{
    collections::HashSet,
    io::Write,
    path::Path,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
};

use async_trait::async_trait;
use audiodown_domain::plugin::PluginId;
use audiodown_plugin_manager::{
    service::{InspectionError, PluginManagerService, PluginStateStore},
    DownloadedSnapshot, PluginManagerError, RepositorySource,
};
use flate2::{write::GzEncoder, Compression};
use serde_json::json;
use tar::Builder;
use tempfile::TempDir;
use tokio::sync::Notify;

const COMMIT_SHA: &str = "0123456789abcdef0123456789abcdef01234567";

#[tokio::test]
async fn owns_the_complete_repository_inspection_flow() {
    let temp = TempDir::new().unwrap();
    let plugin_data = temp.path().join("plugins");
    let expired = plugin_data
        .join("staging")
        .join("65ddab42-9e2f-4de1-a159-705bf9d055e9");
    std::fs::create_dir_all(&expired).unwrap();
    std::fs::write(
        expired.join("snapshot.json"),
        br#"{"createdAt":"2026-07-11T00:00:00Z"}"#,
    )
    .unwrap();

    let source = Arc::new(FixtureSource::valid());
    let state = Arc::new(FakeStateStore::with_installed([
        "com.audiodown.virtual.content",
    ]));
    let service = PluginManagerService::new(
        state.clone(),
        source.clone(),
        plugin_data.clone(),
        "1.0.0-alpha.1".parse().unwrap(),
        "1.0.0".parse().unwrap(),
    );

    let inspected = service
        .inspect_repository_at(
            "https://github.com/example-owner/example-repository",
            "2026-07-11T01:00:00Z".parse().unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(inspected.repository.id, "example.plugins");
    assert_eq!(inspected.repository.name, "Example Plugins");
    assert_eq!(
        inspected.repository.source_url,
        "https://github.com/example-owner/example-repository"
    );
    assert_eq!(inspected.repository.commit_sha, COMMIT_SHA);
    assert_eq!(inspected.plugins.len(), 1);
    assert_eq!(
        inspected.plugins[0].plugin_id.as_str(),
        "com.audiodown.virtual.content"
    );
    assert!(inspected.plugins[0].already_installed);
    assert!(!inspected.plugins[0].requires_lifecycle_script_grant);
    assert!(!expired.exists());
    assert_eq!(source.calls.load(Ordering::SeqCst), 1);
    assert_eq!(state.lookups.lock().unwrap().len(), 1);
    assert!(plugin_data
        .join("staging")
        .join(inspected.snapshot_id.to_string())
        .join("snapshot.json")
        .is_file());
    assert!(plugin_data
        .join("incoming")
        .read_dir()
        .unwrap()
        .next()
        .is_none());
}

#[tokio::test]
async fn maps_repository_failures_to_stable_service_errors() {
    let invalid_url = test_service(Arc::new(FixtureSource::valid()));
    assert!(matches!(
        invalid_url.inspect_repository("not-a-url").await,
        Err(InspectionError::InvalidRepositoryUrl)
    ));

    let unavailable = test_service(Arc::new(FixtureSource::failing()));
    assert!(matches!(
        unavailable
            .inspect_repository("https://github.com/example-owner/example-repository")
            .await,
        Err(InspectionError::RepositoryUnavailable)
    ));

    let invalid = test_service(Arc::new(FixtureSource::invalid_archive()));
    assert!(matches!(
        invalid
            .inspect_repository("https://github.com/example-owner/example-repository")
            .await,
        Err(InspectionError::InvalidRepository)
    ));
}

#[tokio::test]
async fn limits_repository_inspection_to_two_concurrent_fetches() {
    let source = Arc::new(BlockingSource::default());
    let service = Arc::new(test_service(source.clone()));
    let url = "https://github.com/example-owner/example-repository";

    let first = tokio::spawn({
        let service = service.clone();
        async move { service.inspect_repository(url).await }
    });
    let second = tokio::spawn({
        let service = service.clone();
        async move { service.inspect_repository(url).await }
    });

    while source.entered.load(Ordering::SeqCst) < 2 {
        tokio::task::yield_now().await;
    }
    assert!(matches!(
        service.inspect_repository(url).await,
        Err(InspectionError::Busy)
    ));
    assert_eq!(source.entered.load(Ordering::SeqCst), 2);

    source.release.notify_waiters();
    assert!(matches!(
        first.await.unwrap(),
        Err(InspectionError::RepositoryUnavailable)
    ));
    assert!(matches!(
        second.await.unwrap(),
        Err(InspectionError::RepositoryUnavailable)
    ));
}

fn test_service(source: Arc<dyn RepositorySource>) -> PluginManagerService {
    let temp = TempDir::new().unwrap();
    let plugin_data = temp.keep().join("plugins");
    PluginManagerService::new(
        Arc::new(FakeStateStore::default()),
        source,
        plugin_data,
        "1.0.0-alpha.1".parse().unwrap(),
        "1.0.0".parse().unwrap(),
    )
}

#[derive(Default)]
struct FakeStateStore {
    installed: HashSet<String>,
    lookups: Mutex<Vec<String>>,
}

impl FakeStateStore {
    fn with_installed<const N: usize>(plugin_ids: [&str; N]) -> Self {
        Self {
            installed: plugin_ids.into_iter().map(str::to_string).collect(),
            lookups: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl PluginStateStore for FakeStateStore {
    async fn is_installed(&self, plugin_id: &PluginId) -> Result<bool, PluginManagerError> {
        self.lookups
            .lock()
            .unwrap()
            .push(plugin_id.as_str().to_string());
        Ok(self.installed.contains(plugin_id.as_str()))
    }
}

struct FixtureSource {
    archive: Vec<u8>,
    failure: bool,
    calls: AtomicUsize,
}

impl FixtureSource {
    fn valid() -> Self {
        Self {
            archive: valid_repository_archive(),
            failure: false,
            calls: AtomicUsize::new(0),
        }
    }

    fn invalid_archive() -> Self {
        Self {
            archive: b"not a tar archive".to_vec(),
            failure: false,
            calls: AtomicUsize::new(0),
        }
    }

    fn failing() -> Self {
        Self {
            archive: Vec::new(),
            failure: true,
            calls: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl RepositorySource for FixtureSource {
    async fn resolve_and_download(
        &self,
        _source: &audiodown_plugin_manager::github::GitHubRepositoryRef,
        destination: &Path,
    ) -> Result<DownloadedSnapshot, PluginManagerError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.failure {
            return Err(PluginManagerError::RepositoryRequest);
        }
        std::fs::create_dir_all(destination).map_err(|_| PluginManagerError::SnapshotIo)?;
        let archive_path = destination.join("snapshot.tar.gz");
        std::fs::write(&archive_path, &self.archive).map_err(|_| PluginManagerError::SnapshotIo)?;
        Ok(DownloadedSnapshot {
            commit_sha: COMMIT_SHA.to_string(),
            archive_path,
        })
    }
}

#[derive(Default)]
struct BlockingSource {
    entered: AtomicUsize,
    release: Notify,
}

#[async_trait]
impl RepositorySource for BlockingSource {
    async fn resolve_and_download(
        &self,
        _source: &audiodown_plugin_manager::github::GitHubRepositoryRef,
        _destination: &Path,
    ) -> Result<DownloadedSnapshot, PluginManagerError> {
        self.entered.fetch_add(1, Ordering::SeqCst);
        self.release.notified().await;
        Err(PluginManagerError::RepositoryRequest)
    }
}

fn valid_repository_archive() -> Vec<u8> {
    let encoder = GzEncoder::new(Vec::new(), Compression::default());
    let mut archive = Builder::new(encoder);
    append(
        &mut archive,
        "root/audiodown-repository.json",
        &json!({
            "schemaVersion": "1.0",
            "repository": {"id": "example.plugins", "name": "Example Plugins"},
            "plugins": [{"path": "plugins/virtual-content"}]
        })
        .to_string(),
    );
    append(
        &mut archive,
        "root/plugins/virtual-content/audiodown-plugin.json",
        &json!({
            "schemaVersion": "1.0",
            "id": "com.audiodown.virtual.content",
            "name": "Virtual Content",
            "version": "1.0.0",
            "type": "content",
            "runtime": {"type": "nodejs", "version": "22", "entry": "src/index.js"},
            "compatibility": {"pluginApi": ">=1.0 <2.0", "core": ">=1.0 <2.0"},
            "platform": {"id": "virtual", "name": "Virtual"},
            "capabilities": ["content.search"],
            "network": {"allowedHosts": []}
        })
        .to_string(),
    );
    append(
        &mut archive,
        "root/plugins/virtual-content/package.json",
        r#"{"name":"virtual-content","version":"1.0.0"}"#,
    );
    append(
        &mut archive,
        "root/plugins/virtual-content/package-lock.json",
        r#"{"name":"virtual-content","version":"1.0.0","lockfileVersion":3,"packages":{"":{"name":"virtual-content","version":"1.0.0"}}}"#,
    );
    append(
        &mut archive,
        "root/plugins/virtual-content/src/index.js",
        "export default {};\n",
    );
    archive.into_inner().unwrap().finish().unwrap()
}

fn append<W: Write>(archive: &mut Builder<W>, path: &str, content: &str) {
    let mut header = tar::Header::new_gnu();
    header.set_size(content.len() as u64);
    header.set_mode(0o600);
    header.set_cksum();
    archive
        .append_data(&mut header, path, content.as_bytes())
        .unwrap();
}
