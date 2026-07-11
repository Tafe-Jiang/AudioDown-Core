use std::{
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use audiodown_server::supervisor::{SupervisorClient, SupervisorError, UnixSupervisorClient};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::UnixListener,
};

struct TestEndpoint {
    directory: PathBuf,
    socket: PathBuf,
    token: PathBuf,
}

impl TestEndpoint {
    fn new(label: &str) -> Self {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory =
            std::env::temp_dir().join(format!("audiodown-{label}-{}-{unique}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let token = directory.join("core.token");
        std::fs::write(&token, "test-token\n").unwrap();
        Self {
            socket: directory.join("supervisor.sock"),
            token,
            directory,
        }
    }
}

impl Drop for TestEndpoint {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

async fn spawn_response_server(
    socket: &Path,
    response: Vec<u8>,
    delay: Duration,
) -> tokio::task::JoinHandle<()> {
    let listener = UnixListener::bind(socket).unwrap();
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let (reader, mut writer) = stream.into_split();
        let mut request = String::new();
        BufReader::new(reader)
            .read_line(&mut request)
            .await
            .unwrap();
        tokio::time::sleep(delay).await;
        let _ = writer.write_all(&response).await;
    })
}

#[tokio::test]
async fn pings_supervisor_over_unix_socket() {
    let endpoint = TestEndpoint::new("supervisor-success");
    let response =
        br#"{"id":"response-id","ok":true,"result":{"ok":true,"service":"audiodown-supervisor"}}
"#
        .to_vec();
    let server = spawn_response_server(&endpoint.socket, response, Duration::from_millis(0)).await;
    let client = UnixSupervisorClient::new(&endpoint.socket, &endpoint.token);

    let health = client.ping().await.unwrap();
    assert_eq!(health.service, "audiodown-supervisor");
    server.await.unwrap();
}

#[tokio::test]
async fn rejects_malformed_response() {
    let endpoint = TestEndpoint::new("supervisor-malformed");
    let server = spawn_response_server(
        &endpoint.socket,
        b"not-json\n".to_vec(),
        Duration::from_millis(0),
    )
    .await;
    let client = UnixSupervisorClient::new(&endpoint.socket, &endpoint.token);

    assert!(matches!(
        client.ping().await,
        Err(SupervisorError::InvalidResponse)
    ));
    server.await.unwrap();
}

#[tokio::test]
async fn times_out_after_default_two_seconds() {
    let endpoint = TestEndpoint::new("supervisor-timeout");
    let server =
        spawn_response_server(&endpoint.socket, Vec::new(), Duration::from_millis(2_200)).await;
    let client = UnixSupervisorClient::new(&endpoint.socket, &endpoint.token);

    assert!(matches!(client.ping().await, Err(SupervisorError::Timeout)));
    server.await.unwrap();
}

#[tokio::test]
async fn reports_missing_socket_as_unavailable() {
    let endpoint = TestEndpoint::new("supervisor-missing");
    let client = UnixSupervisorClient::new(&endpoint.socket, &endpoint.token);

    assert!(matches!(
        client.ping().await,
        Err(SupervisorError::Unavailable)
    ));
}

#[tokio::test]
async fn rejects_response_larger_than_one_mebibyte() {
    let endpoint = TestEndpoint::new("supervisor-oversized");
    let response = vec![b'x'; 1024 * 1024 + 1];
    let server = spawn_response_server(&endpoint.socket, response, Duration::from_millis(0)).await;
    let client = UnixSupervisorClient::new(&endpoint.socket, &endpoint.token);

    assert!(matches!(
        client.ping().await,
        Err(SupervisorError::ResponseTooLarge)
    ));
    server.await.unwrap();
}
