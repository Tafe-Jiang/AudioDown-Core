use audiodown_domain::credential::{CredentialId, CredentialScope, CredentialStatus};
use audiodown_plugin_api::{
    credential::{
        CredentialAccountStatus, CredentialImportRequest, CredentialImportResult,
        CredentialLogoutRequest, CredentialLogoutResult, CredentialMethod,
        CredentialPromotionRequest, CredentialQrPollRequest, CredentialQrPollResult,
        CredentialQrStartRequest, CredentialQrStartResult, CredentialRefreshRequest,
        CredentialRefreshResult, CredentialStatusRequest, CredentialStatusResult,
        PluginOpaqueState, QrPollStatus, QrPresentation, MAX_PLUGIN_OPAQUE_STATE_BYTES,
        MAX_QR_PAYLOAD_BYTES,
    },
    error::PluginErrorCode,
    manifest::{capability_is_supported, PluginType},
};
use serde_json::json;

fn credential_id() -> CredentialId {
    CredentialId::parse("8d86182f-95f7-44d8-a75c-b9d1ec2c18ad").unwrap()
}

fn scope() -> CredentialScope {
    CredentialScope::parse("virtual.web").unwrap()
}

fn account_status() -> CredentialAccountStatus {
    CredentialAccountStatus {
        status: CredentialStatus::Active,
        account_id_hint: Some("virtual-account-1".to_owned()),
        display_name: Some("Virtual Account".to_owned()),
    }
}

#[test]
fn serializes_only_the_six_phase_four_methods() {
    let methods = [
        (CredentialMethod::QrStart, "\"credential.qr.start\""),
        (CredentialMethod::QrPoll, "\"credential.qr.poll\""),
        (CredentialMethod::Import, "\"credential.import\""),
        (CredentialMethod::Status, "\"credential.status\""),
        (CredentialMethod::Refresh, "\"credential.refresh\""),
        (CredentialMethod::Logout, "\"credential.logout\""),
    ];

    for (method, encoded) in methods {
        assert_eq!(serde_json::to_string(&method).unwrap(), encoded);
        assert_eq!(
            serde_json::from_str::<CredentialMethod>(encoded).unwrap(),
            method
        );
        assert_eq!(method.capability(), encoded.trim_matches('"'));
        assert_eq!(
            CredentialMethod::from_capability(method.capability()),
            Some(method)
        );
        assert!(capability_is_supported(
            PluginType::Credential,
            method.capability()
        ));
        assert!(!capability_is_supported(
            PluginType::Content,
            method.capability()
        ));
    }

    assert!(serde_json::from_str::<CredentialMethod>("\"credential.export\"").is_err());
    assert!(serde_json::from_str::<CredentialMethod>("\"content.search\"").is_err());
}

#[test]
fn round_trips_declarative_qr_start_and_poll_contracts() {
    let request: CredentialQrStartRequest = serde_json::from_value(json!({
        "scope": "virtual.web",
        "cookieJarSessionId": "jar-session-1"
    }))
    .unwrap();
    request.validate().unwrap();

    let result = CredentialQrStartResult {
        presentation: QrPresentation {
            payload: "virtual-qr-payload".to_owned(),
            display_code: Some("ABCD-1234".to_owned()),
            expires_in_seconds: 300,
            poll_interval_seconds: 2,
            plugin_state: Some(PluginOpaqueState::parse("state-1").unwrap()),
        },
    };
    result.validate().unwrap();
    let encoded = serde_json::to_value(&result).unwrap();
    assert_eq!(encoded["presentation"]["payload"], "virtual-qr-payload");
    assert_eq!(encoded["presentation"]["pollIntervalSeconds"], 2);
    assert!(encoded["presentation"].get("html").is_none());

    let poll_request: CredentialQrPollRequest = serde_json::from_value(json!({
        "scope": "virtual.web",
        "cookieJarSessionId": "jar-session-1",
        "pluginState": "state-1"
    }))
    .unwrap();
    poll_request.validate().unwrap();

    for status in [
        QrPollStatus::Pending,
        QrPollStatus::Scanned,
        QrPollStatus::Confirmed,
        QrPollStatus::Expired,
        QrPollStatus::Denied,
    ] {
        serde_json::from_value::<QrPollStatus>(serde_json::to_value(status).unwrap()).unwrap();
    }

    let poll_result = CredentialQrPollResult {
        status: QrPollStatus::Confirmed,
        next_poll_seconds: None,
        plugin_state: None,
        promotion: Some(CredentialPromotionRequest { scope: scope() }),
        account: Some(account_status()),
    };
    poll_result.validate().unwrap();
    assert_eq!(
        serde_json::to_value(&poll_result).unwrap()["promotion"]["scope"],
        "virtual.web"
    );

    let invalid_promotion = CredentialQrPollResult {
        status: QrPollStatus::Pending,
        next_poll_seconds: Some(2),
        plugin_state: None,
        promotion: Some(CredentialPromotionRequest { scope: scope() }),
        account: None,
    };
    assert!(invalid_promotion.validate().is_err());

    let invalid_confirmed_account = CredentialQrPollResult {
        status: QrPollStatus::Confirmed,
        next_poll_seconds: None,
        plugin_state: None,
        promotion: Some(CredentialPromotionRequest { scope: scope() }),
        account: Some(CredentialAccountStatus {
            status: CredentialStatus::Expired,
            account_id_hint: None,
            display_name: None,
        }),
    };
    assert!(invalid_confirmed_account.validate().is_err());
}

#[test]
fn round_trips_import_status_refresh_and_logout_without_plaintext() {
    let import: CredentialImportRequest = serde_json::from_value(json!({
        "credentialId": credential_id(),
        "scope": "virtual.web"
    }))
    .unwrap();
    import.validate().unwrap();
    let import_result = CredentialImportResult {
        account: account_status(),
    };
    import_result.validate().unwrap();

    let status: CredentialStatusRequest = serde_json::from_value(json!({
        "credentialId": credential_id(),
        "scope": "virtual.web"
    }))
    .unwrap();
    status.validate().unwrap();
    CredentialStatusResult {
        account: account_status(),
    }
    .validate()
    .unwrap();

    let refresh: CredentialRefreshRequest = serde_json::from_value(json!({
        "credentialId": credential_id(),
        "scope": "virtual.web",
        "cookieJarSessionId": "refresh-jar-1"
    }))
    .unwrap();
    refresh.validate().unwrap();
    CredentialRefreshResult {
        account: account_status(),
    }
    .validate()
    .unwrap();
    assert!(CredentialRefreshResult {
        account: CredentialAccountStatus {
            status: CredentialStatus::Error,
            account_id_hint: None,
            display_name: None,
        },
    }
    .validate()
    .is_err());

    let logout: CredentialLogoutRequest = serde_json::from_value(json!({
        "credentialId": credential_id(),
        "scope": "virtual.web"
    }))
    .unwrap();
    logout.validate().unwrap();
    CredentialLogoutResult {
        status: CredentialStatus::Revoked,
    }
    .validate()
    .unwrap();

    let serialized = serde_json::to_string(&(import, import_result, status, refresh, logout))
        .unwrap()
        .to_ascii_lowercase();
    for forbidden in [
        "cookievalue",
        "set-cookie",
        "authorization",
        "proxytoken",
        "ciphertext",
        "nonce",
        "masterkey",
    ] {
        assert!(
            !serialized.contains(forbidden),
            "credential RPC leaked forbidden field {forbidden}"
        );
    }
}

#[test]
fn rejects_plaintext_executable_markup_and_runtime_controls() {
    for forbidden in [
        json!({
            "credentialId": credential_id(),
            "scope": "virtual.web",
            "cookie": "plaintext-cookie"
        }),
        json!({
            "credentialId": credential_id(),
            "scope": "virtual.web",
            "headers": {"Authorization": "secret"}
        }),
        json!({
            "credentialId": credential_id(),
            "scope": "virtual.web",
            "proxyToken": "secret"
        }),
        json!({
            "credentialId": credential_id(),
            "scope": "virtual.web",
            "dockerImage": "caller-controlled"
        }),
    ] {
        assert!(serde_json::from_value::<CredentialImportRequest>(forbidden).is_err());
    }

    for forbidden in [
        json!({
            "presentation": {
                "payload": "virtual",
                "expiresInSeconds": 300,
                "pollIntervalSeconds": 2,
                "html": "<script>unsafe</script>"
            }
        }),
        json!({
            "presentation": {
                "payload": "virtual",
                "expiresInSeconds": 300,
                "pollIntervalSeconds": 2,
                "script": "unsafe()"
            }
        }),
    ] {
        assert!(serde_json::from_value::<CredentialQrStartResult>(forbidden).is_err());
    }
}

#[test]
fn rejects_oversized_or_inconsistent_credential_values() {
    assert!(PluginOpaqueState::parse("x".repeat(MAX_PLUGIN_OPAQUE_STATE_BYTES + 1)).is_err());

    let oversized_qr = CredentialQrStartResult {
        presentation: QrPresentation {
            payload: "x".repeat(MAX_QR_PAYLOAD_BYTES + 1),
            display_code: None,
            expires_in_seconds: 300,
            poll_interval_seconds: 2,
            plugin_state: None,
        },
    };
    assert!(oversized_qr.validate().is_err());

    let missing_next_poll = CredentialQrPollResult {
        status: QrPollStatus::Scanned,
        next_poll_seconds: None,
        plugin_state: None,
        promotion: None,
        account: None,
    };
    assert!(missing_next_poll.validate().is_err());

    let unexpected_next_poll = CredentialQrPollResult {
        status: QrPollStatus::Expired,
        next_poll_seconds: Some(2),
        plugin_state: None,
        promotion: None,
        account: None,
    };
    assert!(unexpected_next_poll.validate().is_err());
}

#[test]
fn exposes_the_phase_four_standard_error_codes() {
    let cases = [
        (PluginErrorCode::InvalidRequest, "INVALID_REQUEST"),
        (PluginErrorCode::PluginNotFound, "PLUGIN_NOT_FOUND"),
        (PluginErrorCode::PluginDisabled, "PLUGIN_DISABLED"),
        (
            PluginErrorCode::PluginCapabilityMissing,
            "PLUGIN_CAPABILITY_MISSING",
        ),
        (PluginErrorCode::PluginUnavailable, "PLUGIN_UNAVAILABLE"),
        (PluginErrorCode::PluginTimeout, "PLUGIN_TIMEOUT"),
        (
            PluginErrorCode::PluginResponseInvalid,
            "PLUGIN_RESPONSE_INVALID",
        ),
        (PluginErrorCode::CredentialNotFound, "CREDENTIAL_NOT_FOUND"),
        (PluginErrorCode::CredentialExpired, "CREDENTIAL_EXPIRED"),
        (
            PluginErrorCode::CredentialScopeNotAllowed,
            "CREDENTIAL_SCOPE_NOT_ALLOWED",
        ),
        (PluginErrorCode::LoginFlowNotFound, "LOGIN_FLOW_NOT_FOUND"),
        (PluginErrorCode::LoginFlowExpired, "LOGIN_FLOW_EXPIRED"),
        (PluginErrorCode::LoginPending, "LOGIN_PENDING"),
        (PluginErrorCode::LoginDenied, "LOGIN_DENIED"),
        (PluginErrorCode::RateLimited, "RATE_LIMITED"),
        (
            PluginErrorCode::PlatformResponseChanged,
            "PLATFORM_RESPONSE_CHANGED",
        ),
        (
            PluginErrorCode::PluginInternalError,
            "PLUGIN_INTERNAL_ERROR",
        ),
    ];

    for (code, expected) in cases {
        assert_eq!(
            serde_json::to_value(code).unwrap(),
            serde_json::Value::String(expected.to_owned())
        );
    }

    assert!(PluginErrorCode::LoginPending.is_retryable());
    assert!(PluginErrorCode::RateLimited.is_retryable());
    assert!(!PluginErrorCode::LoginDenied.is_retryable());
    assert!(!PluginErrorCode::CredentialScopeNotAllowed.is_retryable());
}
