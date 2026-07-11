use std::{
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use audiodown_domain::plugin::PluginId;
use audiodown_plugin_api::content::ContentMethod;
use audiodown_server::supervisor::{SupervisorClient, SupervisorError, UnixSupervisorClient};
use audiodown_supervisor_protocol::{
    PluginInstallOperation, PluginInstallOperationState, PluginRemoveResult, PluginRpcResult,
};
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
            PathBuf::from("/tmp").join(format!("ad-{label}-{}-{unique}", std::process::id()));
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

#[tokio::test]
async fn sends_only_fixed_install_operation_parameters() {
    let endpoint = TestEndpoint::new("supervisor-install-contract");
    let listener = UnixListener::bind(&endpoint.socket).unwrap();
    let operation_id = uuid::Uuid::new_v4();
    let plugin_id = PluginId::parse("com.audiodown.virtual.content").unwrap();
    let expected_plugin = plugin_id.clone();
    let server = tokio::spawn(async move {
        let mut requests = Vec::new();
        for _ in 0..7 {
            let (stream, _) = listener.accept().await.unwrap();
            let (reader, mut writer) = stream.into_split();
            let mut request = String::new();
            BufReader::new(reader)
                .read_line(&mut request)
                .await
                .unwrap();
            let request: serde_json::Value = serde_json::from_str(&request).unwrap();
            let method = request["method"].as_str().unwrap();
            let result = match method {
                "plugin.install.list" => serde_json::json!({"operations": []}),
                "plugin.remove" => serde_json::json!({
                    "pluginId": expected_plugin,
                    "removedContainer": false,
                    "removedImage": false,
                    "removedInstallDirectory": false
                }),
                _ => serde_json::json!({
                    "operationId": operation_id,
                    "pluginId": expected_plugin,
                    "state": "accepted",
                    "artifact": null,
                    "buildLogs": [],
                    "errorCode": null,
                    "acknowledged": false
                }),
            };
            let response = serde_json::json!({
                "id": request["id"],
                "ok": true,
                "result": result
            });
            writer
                .write_all(format!("{response}\n").as_bytes())
                .await
                .unwrap();
            requests.push(request);
        }
        requests
    });
    let client = UnixSupervisorClient::new(&endpoint.socket, &endpoint.token);

    let operation: PluginInstallOperation = client
        .begin_plugin_install(&plugin_id, operation_id)
        .await
        .unwrap();
    assert_eq!(operation.state, PluginInstallOperationState::Accepted);
    client
        .plugin_install_status(&plugin_id, operation_id)
        .await
        .unwrap();
    client
        .finalize_plugin_install(&plugin_id, operation_id)
        .await
        .unwrap();
    client
        .abort_plugin_install(&plugin_id, operation_id)
        .await
        .unwrap();
    client
        .acknowledge_plugin_install(&plugin_id, operation_id)
        .await
        .unwrap();
    let list = client.list_plugin_install_operations().await.unwrap();
    assert!(list.operations.is_empty());
    let removed: PluginRemoveResult = client.remove_plugin(&plugin_id).await.unwrap();
    assert!(!removed.removed_image);

    let requests = server.await.unwrap();
    assert_eq!(
        requests
            .iter()
            .map(|request| request["method"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [
            "plugin.install.build",
            "plugin.install.status",
            "plugin.install.finalize",
            "plugin.install.abort",
            "plugin.install.ack",
            "plugin.install.list",
            "plugin.remove",
        ]
    );
    for request in &requests[..5] {
        assert_eq!(
            request["params"],
            serde_json::json!({
                "pluginId": plugin_id,
                "operationId": operation_id
            })
        );
        assert_eq!(request["params"].as_object().unwrap().len(), 2);
    }
    assert!(requests[5].get("params").is_none());
    assert_eq!(
        requests[6]["params"],
        serde_json::json!({"pluginId": plugin_id})
    );
}

#[tokio::test]
async fn sends_only_typed_content_rpc_parameters() {
    let endpoint = TestEndpoint::new("supervisor-content-rpc");
    let listener = UnixListener::bind(&endpoint.socket).unwrap();
    let plugin_id = PluginId::parse("com.audiodown.virtual.content").unwrap();
    let expected_plugin = plugin_id.clone();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let (reader, mut writer) = stream.into_split();
        let mut request = String::new();
        BufReader::new(reader)
            .read_line(&mut request)
            .await
            .unwrap();
        let request: serde_json::Value = serde_json::from_str(&request).unwrap();
        let response = serde_json::json!({
            "id": request["id"],
            "ok": true,
            "result": {
                "response": {
                    "jsonrpc": "2.0",
                    "id": "plugin-request",
                    "result": {"items": [], "nextCursor": null}
                }
            }
        });
        writer
            .write_all(format!("{response}\n").as_bytes())
            .await
            .unwrap();
        (request, expected_plugin)
    });
    let client = UnixSupervisorClient::new(&endpoint.socket, &endpoint.token);

    let result: PluginRpcResult = client
        .invoke_plugin(
            &plugin_id,
            ContentMethod::Search,
            serde_json::json!({"query": "virtual", "limit": 20}),
        )
        .await
        .unwrap();
    assert_eq!(result.response.id, "plugin-request");

    let (request, expected_plugin) = server.await.unwrap();
    assert_eq!(request["method"], "plugin.rpc");
    assert_eq!(
        request["params"],
        serde_json::json!({
            "pluginId": expected_plugin,
            "method": "content.search",
            "params": {"query": "virtual", "limit": 20}
        })
    );
    assert_eq!(request["params"].as_object().unwrap().len(), 3);
}
