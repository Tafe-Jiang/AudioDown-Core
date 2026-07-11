use std::sync::{Arc, Mutex};

use audiodown_plugin_manager::{
    github::{GitHubClient, GitHubRepositoryRef},
    PluginManagerError, RepositorySource,
};
use axum::{
    body::Body,
    extract::{Request, State},
    http::{header, Response, StatusCode},
    response::IntoResponse,
    Router,
};
use serde_json::json;
use tempfile::TempDir;
use tokio::{net::TcpListener, task::JoinHandle};

const COMMIT_SHA: &str = "0123456789abcdef0123456789abcdef01234567";
const MAX_ARCHIVE_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone)]
struct Fixture {
    requests: Arc<Mutex<Vec<String>>>,
    repository_status: StatusCode,
    repository_body: Vec<u8>,
    commit_status: StatusCode,
    commit_body: Vec<u8>,
    archive_status: StatusCode,
    archive_body: Vec<u8>,
    redirect_location: Option<String>,
}

impl Default for Fixture {
    fn default() -> Self {
        Self {
            requests: Arc::new(Mutex::new(Vec::new())),
            repository_status: StatusCode::OK,
            repository_body: serde_json::to_vec(&json!({
                "default_branch": "main",
                "tarball_url": "https://example.invalid/untrusted-repository-archive"
            }))
            .unwrap(),
            commit_status: StatusCode::OK,
            commit_body: serde_json::to_vec(&json!({
                "sha": COMMIT_SHA,
                "tarball_url": "https://example.invalid/untrusted-commit-archive"
            }))
            .unwrap(),
            archive_status: StatusCode::OK,
            archive_body: b"ok".to_vec(),
            redirect_location: None,
        }
    }
}

#[tokio::test]
async fn resolves_default_branch_to_commit_before_downloading() {
    let fixture = Fixture::default();
    let requests = fixture.requests.clone();
    let server = TestServer::spawn(fixture).await;
    let temp = TempDir::new().unwrap();
    let source =
        GitHubRepositoryRef::parse("https://github.com/example-owner/example-repository").unwrap();
    let client = GitHubClient::new(&server.base_url, &server.base_url).unwrap();

    let result = client
        .resolve_and_download(&source, temp.path())
        .await
        .unwrap();

    assert_eq!(result.commit_sha, COMMIT_SHA);
    assert_eq!(tokio::fs::read(result.archive_path).await.unwrap(), b"ok");
    assert_eq!(
        requests.lock().unwrap().as_slice(),
        [
            "/repos/example-owner/example-repository",
            "/repos/example-owner/example-repository/commits/main",
            "/example-owner/example-repository/tar.gz/0123456789abcdef0123456789abcdef01234567",
        ]
    );
    assert!(!temp.path().join("snapshot.tar.gz.tmp").exists());
}

#[tokio::test]
async fn rejects_redirects_and_non_success_responses() {
    let redirect = Fixture {
        repository_status: StatusCode::FOUND,
        redirect_location: Some("https://example.invalid/redirect".to_string()),
        ..Fixture::default()
    };
    assert_download_fails(redirect).await;

    let repository_error = Fixture {
        repository_status: StatusCode::INTERNAL_SERVER_ERROR,
        ..Fixture::default()
    };
    assert_download_fails(repository_error).await;

    let archive_error = Fixture {
        archive_status: StatusCode::BAD_GATEWAY,
        ..Fixture::default()
    };
    assert_download_fails(archive_error).await;
}

#[tokio::test]
async fn rejects_missing_default_branches_and_invalid_commit_shas() {
    let missing_branch = Fixture {
        repository_body: serde_json::to_vec(&json!({})).unwrap(),
        ..Fixture::default()
    };
    assert!(matches!(
        download_result(missing_branch).await,
        Err(PluginManagerError::MissingDefaultBranch)
    ));

    for sha in [
        "0123456789abcdef",
        "0123456789ABCDEF0123456789ABCDEF01234567",
        "g123456789abcdef0123456789abcdef01234567",
    ] {
        let invalid_sha = Fixture {
            commit_body: serde_json::to_vec(&json!({"sha": sha})).unwrap(),
            ..Fixture::default()
        };
        assert_download_fails(invalid_sha).await;
    }
}

#[tokio::test]
async fn rejects_archives_larger_than_sixteen_mebibytes() {
    let fixture = Fixture {
        archive_body: vec![b'x'; MAX_ARCHIVE_BYTES + 1],
        ..Fixture::default()
    };
    assert_download_fails(fixture).await;
}

async fn assert_download_fails(fixture: Fixture) {
    assert!(download_result(fixture).await.is_err());
}

async fn download_result(
    fixture: Fixture,
) -> Result<audiodown_plugin_manager::DownloadedSnapshot, PluginManagerError> {
    let server = TestServer::spawn(fixture).await;
    let temp = TempDir::new().unwrap();
    let source =
        GitHubRepositoryRef::parse("https://github.com/example-owner/example-repository").unwrap();
    let client = GitHubClient::new(&server.base_url, &server.base_url).unwrap();

    let result = client.resolve_and_download(&source, temp.path()).await;
    assert!(!temp.path().join("snapshot.tar.gz").exists());
    assert!(!temp.path().join("snapshot.tar.gz.tmp").exists());
    result
}

struct TestServer {
    base_url: String,
    task: JoinHandle<()>,
}

impl TestServer {
    async fn spawn(fixture: Fixture) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = Router::new().fallback(handle_request).with_state(fixture);
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        Self {
            base_url: format!("http://{address}"),
            task,
        }
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn handle_request(State(fixture): State<Fixture>, request: Request) -> Response<Body> {
    let path = request.uri().path().to_string();
    fixture.requests.lock().unwrap().push(path.clone());

    match path.as_str() {
        "/repos/example-owner/example-repository" => {
            if let Some(location) = fixture.redirect_location {
                return Response::builder()
                    .status(fixture.repository_status)
                    .header(header::LOCATION, location)
                    .body(Body::empty())
                    .unwrap();
            }
            response(
                fixture.repository_status,
                "application/json",
                fixture.repository_body,
            )
        }
        "/repos/example-owner/example-repository/commits/main" => response(
            fixture.commit_status,
            "application/json",
            fixture.commit_body,
        ),
        path if path == format!("/example-owner/example-repository/tar.gz/{COMMIT_SHA}") => {
            response(
                fixture.archive_status,
                "application/octet-stream",
                fixture.archive_body,
            )
        }
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

fn response(status: StatusCode, content_type: &str, body: Vec<u8>) -> Response<Body> {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, content_type)
        .body(Body::from(body))
        .unwrap()
}
