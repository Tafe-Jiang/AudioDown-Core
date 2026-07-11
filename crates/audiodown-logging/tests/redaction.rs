use audiodown_logging::{redact_json, redact_text};

#[test]
fn redacts_sensitive_text() {
    let input =
        r#"Cookie: session=secret123; Authorization: Bearer token456; phone=13800138000"#;
    let output = redact_text(input);

    assert!(!output.contains("secret123"));
    assert!(!output.contains("token456"));
    assert!(!output.contains("13800138000"));
    assert!(output.contains("[REDACTED]"));
}

#[test]
fn recursively_redacts_sensitive_json_keys() {
    let input = serde_json::json!({
        "cookie": "session=secret123",
        "Authorization": "Bearer token456",
        "nested": {
            "TOKEN": "token789",
            "password": "password-value",
            "items": [
                {"secret": "secret-value"},
                {"Set-Cookie": "session=server-secret"}
            ]
        },
        "safe": "visible"
    });

    let output = redact_json(&input);
    let serialized = output.to_string();

    for secret in [
        "secret123",
        "token456",
        "token789",
        "password-value",
        "secret-value",
        "server-secret",
    ] {
        assert!(!serialized.contains(secret));
    }
    assert_eq!(output["safe"], "visible");
    assert_eq!(output["nested"]["items"][0]["secret"], "[REDACTED]");
}
