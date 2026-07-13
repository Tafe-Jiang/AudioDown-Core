use audiodown_domain::{
    credential::{
        CredentialId, CredentialKind, CredentialOwnership, CredentialPublicMetadata,
        CredentialScope, CredentialStatus, MAX_CREDENTIAL_SCOPE_BYTES,
        MAX_CREDENTIAL_SCOPE_SEGMENTS,
    },
    plugin::PluginId,
};
use chrono::{TimeZone, Utc};
use uuid::Uuid;

#[test]
fn accepts_bounded_lowercase_dotted_scopes() {
    for value in [
        "virtual.web",
        "virtual.account.primary",
        "v1.account2.read3",
    ] {
        let scope = CredentialScope::parse(value).expect("valid scope");
        assert_eq!(scope.as_str(), value);

        let encoded = serde_json::to_string(&scope).expect("serialize scope");
        let decoded: CredentialScope = serde_json::from_str(&encoded).expect("deserialize scope");
        assert_eq!(decoded, scope);
    }
}

#[test]
fn rejects_malformed_or_oversized_scopes() {
    let too_many_segments = std::iter::repeat_n("segment", MAX_CREDENTIAL_SCOPE_SEGMENTS + 1)
        .collect::<Vec<_>>()
        .join(".");
    let too_long = format!("a.{}", "b".repeat(MAX_CREDENTIAL_SCOPE_BYTES));

    for value in [
        "",
        "virtual",
        "Virtual.web",
        "virtual web",
        "virtual/web",
        r"virtual\web",
        "https://virtual.invalid",
        ".virtual.web",
        "virtual.web.",
        "virtual..web",
        "virtual.-web",
        "virtual.web-name",
        "virtual.web_name",
        "virtual.1web",
        &too_many_segments,
        &too_long,
    ] {
        assert!(
            CredentialScope::parse(value).is_err(),
            "scope should be rejected: {value:?}"
        );
    }
}

#[test]
fn credential_ids_are_non_nil_uuid_identities() {
    let value = Uuid::parse_str("8d86182f-95f7-44d8-a75c-b9d1ec2c18ad").unwrap();
    let id = CredentialId::from_uuid(value).expect("non-nil credential ID");

    assert_eq!(id.as_uuid(), value);
    assert_eq!(id.to_string(), value.to_string());
    assert_eq!(
        CredentialId::parse(value.to_string()).unwrap(),
        id,
        "text parsing should preserve identity"
    );

    let encoded = serde_json::to_string(&id).expect("serialize credential ID");
    assert_eq!(encoded, format!("\"{value}\""));
    let decoded: CredentialId = serde_json::from_str(&encoded).expect("deserialize credential ID");
    assert_eq!(decoded, id);

    assert!(CredentialId::from_uuid(Uuid::nil()).is_err());
    assert!(CredentialId::parse(Uuid::nil().to_string()).is_err());
    assert!(CredentialId::parse("not-a-uuid").is_err());
}

#[test]
fn ownership_distinguishes_plugin_owned_and_retained_credentials() {
    let plugin_id = PluginId::parse("com.audiodown.virtual.credential").unwrap();
    let owned = CredentialOwnership::Plugin(plugin_id.clone());
    let retained = CredentialOwnership::Retained;

    assert_eq!(owned.source_plugin_id(), Some(&plugin_id));
    assert_eq!(retained.source_plugin_id(), None);
    assert!(!owned.is_retained());
    assert!(retained.is_retained());
}

#[test]
fn public_metadata_exposes_only_safe_identity_and_status_fields() {
    let timestamp = Utc
        .with_ymd_and_hms(2026, 7, 13, 12, 0, 0)
        .single()
        .unwrap();
    let metadata = CredentialPublicMetadata {
        id: CredentialId::parse("8d86182f-95f7-44d8-a75c-b9d1ec2c18ad").unwrap(),
        kind: CredentialKind::Cookie,
        platform_id: "virtual".to_owned(),
        scope: CredentialScope::parse("virtual.web").unwrap(),
        ownership: CredentialOwnership::Plugin(
            PluginId::parse("com.audiodown.virtual.credential").unwrap(),
        ),
        status: CredentialStatus::Active,
        expires_at: Some(timestamp),
        created_at: timestamp,
        updated_at: timestamp,
    };

    let value = serde_json::to_value(&metadata).expect("serialize public metadata");
    assert_eq!(value["kind"], "cookie");
    assert_eq!(value["scope"], "virtual.web");
    assert_eq!(value["status"], "active");

    let serialized = value.to_string().to_ascii_lowercase();
    for forbidden in [
        "cookievalue",
        "tokenvalue",
        "plaintext",
        "ciphertext",
        "nonce",
        "keyversion",
        "authorization",
    ] {
        assert!(
            !serialized.contains(forbidden),
            "public metadata leaked forbidden field {forbidden}"
        );
    }
}

#[test]
fn credential_statuses_and_kinds_use_stable_safe_names() {
    for (status, expected) in [
        (CredentialStatus::Active, "\"active\""),
        (CredentialStatus::Expired, "\"expired\""),
        (CredentialStatus::Revoked, "\"revoked\""),
        (CredentialStatus::Error, "\"error\""),
    ] {
        assert_eq!(serde_json::to_string(&status).unwrap(), expected);
    }

    assert_eq!(
        serde_json::to_string(&CredentialKind::Cookie).unwrap(),
        "\"cookie\""
    );
    assert_eq!(
        serde_json::to_string(&CredentialKind::Token).unwrap(),
        "\"token\""
    );
}
