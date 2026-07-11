use audiodown_domain::plugin::PluginId;
use audiodown_plugin_api::content::ContentMethod;
use audiodown_supervisor::docker::{
    build_content_rpc_exec, parse_content_rpc_output, DockerAdapterError, PLUGIN_RPC_TIMEOUT,
};

#[test]
fn builds_only_the_fixed_content_rpc_exec_command() {
    let plugin_id = PluginId::parse("com.audiodown.virtual.content").unwrap();
    let plan = build_content_rpc_exec(
        &plugin_id,
        ContentMethod::Search,
        serde_json::json!({"query": "virtual", "limit": 20}),
        "request-1",
    )
    .unwrap();

    assert_eq!(PLUGIN_RPC_TIMEOUT.as_secs(), 8);
    assert_eq!(plan.user, "10002:10002");
    assert_eq!(plan.working_dir, "/tmp");
    assert_eq!(plan.command[0], "node");
    assert_eq!(plan.command[1], "-e");
    assert_eq!(plan.command[3], "/tmp/audiodown-rpc.sock");
    assert_eq!(plan.command[5], "request-1");
    assert_eq!(plan.command.len(), 6);
    assert!(plan.command[2].contains("net.createConnection"));
    assert!(!plan.command[2].contains("child_process"));
    assert!(!plan.command[2].contains("eval("));

    let request: serde_json::Value = serde_json::from_str(&plan.command[4]).unwrap();
    assert_eq!(
        request,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": "request-1",
            "method": "content.search",
            "params": {"query": "virtual", "limit": 20}
        })
    );
    assert_eq!(request.as_object().unwrap().len(), 4);
}

#[test]
fn maps_all_content_methods_without_caller_controlled_runtime_fields() {
    let plugin_id = PluginId::parse("com.audiodown.virtual.content").unwrap();
    for (method, expected, params) in [
        (
            ContentMethod::Search,
            "content.search",
            serde_json::json!({"query": "virtual", "limit": 20}),
        ),
        (
            ContentMethod::Discover,
            "content.discover",
            serde_json::json!({"limit": 20}),
        ),
        (
            ContentMethod::Categories,
            "content.categories",
            serde_json::json!({}),
        ),
        (
            ContentMethod::AlbumGet,
            "content.album.get",
            serde_json::json!({"resourceId": "album-1"}),
        ),
        (
            ContentMethod::TracksList,
            "content.tracks.list",
            serde_json::json!({"albumResourceId": "album-1", "limit": 20}),
        ),
    ] {
        let plan = build_content_rpc_exec(&plugin_id, method, params, "request-method").unwrap();
        let request: serde_json::Value = serde_json::from_str(&plan.command[4]).unwrap();
        assert_eq!(request["method"], expected);
        assert!(request.get("timeout").is_none());
        assert!(request.get("containerId").is_none());
        assert!(request.get("socketPath").is_none());
        assert!(request.get("command").is_none());
    }
}

#[test]
fn parses_one_matching_json_rpc_response_line() {
    let output = br#"{"jsonrpc":"2.0","id":"request-1","result":{"items":[]}}
"#;
    let result = parse_content_rpc_output("request-1", output, b"", 0).unwrap();

    assert_eq!(result.response.id, "request-1");
    assert_eq!(
        result.response.result.unwrap(),
        serde_json::json!({"items": []})
    );
}

#[test]
fn rejects_mismatched_multiple_malformed_and_oversized_responses() {
    for output in [
        br#"{"jsonrpc":"2.0","id":"other","result":{}}
"#
        .as_slice(),
        br#"{"jsonrpc":"2.0","id":"request-1","result":{}}
{"jsonrpc":"2.0","id":"request-1","result":{}}
"#
        .as_slice(),
        b"not-json\n".as_slice(),
        br#"{"jsonrpc":"2.0","id":"request-1","result":{},"error":{"code":-32000,"message":"bad"}}
"#
        .as_slice(),
    ] {
        assert!(matches!(
            parse_content_rpc_output("request-1", output, b"", 0),
            Err(DockerAdapterError::InvalidRpcResponse)
        ));
    }

    assert!(matches!(
        parse_content_rpc_output("request-1", &vec![b'x'; 1024 * 1024 + 1], b"", 0),
        Err(DockerAdapterError::RpcResponseTooLarge)
    ));
}

#[test]
fn rejects_stderr_and_nonzero_exec_status() {
    let valid = br#"{"jsonrpc":"2.0","id":"request-1","result":{}}
"#;
    assert!(matches!(
        parse_content_rpc_output("request-1", valid, b"plugin stderr", 0),
        Err(DockerAdapterError::RpcStderr)
    ));
    assert!(matches!(
        parse_content_rpc_output("request-1", valid, b"", 1),
        Err(DockerAdapterError::RpcExecFailed)
    ));
}
