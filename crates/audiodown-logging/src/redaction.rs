use regex::{Captures, Regex};

const REDACTED: &str = "[REDACTED]";

pub fn redact_text(input: &str) -> String {
    let authorization = Regex::new(r"(?i)(authorization\s*:\s*)[^;\r\n]+")
        .expect("authorization redaction regex must compile");
    let cookie =
        Regex::new(r"(?i)(cookie\s*:\s*)[^;\r\n]+").expect("cookie redaction regex must compile");
    let bearer = Regex::new(r"(?i)\bbearer\s+[a-z0-9._~+/=-]+")
        .expect("bearer redaction regex must compile");
    let mobile = Regex::new(r"\b1[3-9][0-9]{9}\b").expect("mobile redaction regex must compile");
    let query_secret = Regex::new(r"(?i)([?&](?:token|access_token|password|secret)=)[^&#\s]*")
        .expect("query secret redaction regex must compile");

    let output = authorization
        .replace_all(input, |captures: &Captures<'_>| {
            format!("{}{}", &captures[1], REDACTED)
        })
        .into_owned();
    let output = cookie
        .replace_all(&output, |captures: &Captures<'_>| {
            format!("{}{}", &captures[1], REDACTED)
        })
        .into_owned();
    let output = bearer.replace_all(&output, REDACTED).into_owned();
    let output = mobile.replace_all(&output, REDACTED).into_owned();

    query_secret
        .replace_all(&output, |captures: &Captures<'_>| {
            format!("{}{}", &captures[1], REDACTED)
        })
        .into_owned()
}

pub fn redact_json(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(object) => {
            let redacted = object
                .iter()
                .map(|(key, value)| {
                    let value = if is_sensitive_key(key) {
                        serde_json::Value::String(REDACTED.to_string())
                    } else {
                        redact_json(value)
                    };
                    (key.clone(), value)
                })
                .collect();
            serde_json::Value::Object(redacted)
        }
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.iter().map(redact_json).collect())
        }
        serde_json::Value::String(text) => serde_json::Value::String(redact_text(text)),
        other => other.clone(),
    }
}

fn is_sensitive_key(key: &str) -> bool {
    matches!(
        key.to_ascii_lowercase().as_str(),
        "cookie" | "authorization" | "token" | "password" | "secret" | "set-cookie"
    )
}
