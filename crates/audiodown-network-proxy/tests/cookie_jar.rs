use std::time::Duration;

use audiodown_domain::{credential::CredentialScope, plugin::PluginId};
use audiodown_network_proxy::cookie_jar::{CookieJarBinding, CookieJarError, TemporaryCookieJars};
use audiodown_plugin_api::manifest::CredentialTargetOrigin;
use http::{header, HeaderMap, HeaderValue};
use url::Url;

const COOKIE_CANARY: &str = "cookie-jar-canary-must-remain-secret";

#[test]
fn sessions_are_random_expiring_and_bound_to_the_complete_flow_identity() {
    let jars = TemporaryCookieJars::new();
    let binding = login_binding();
    let first = jars
        .create(binding.clone(), Duration::from_secs(60))
        .expect("first session");
    let second = jars
        .create(binding.clone(), Duration::from_secs(60))
        .expect("second session");

    assert_ne!(first, second);
    assert!(!first.as_uuid().is_nil());

    let wrong_plugin = CookieJarBinding::login(
        PluginId::parse("com.example.other.credential").unwrap(),
        "virtual",
        scope(),
        origins(&["https://account.virtual.invalid"]),
    )
    .unwrap();
    assert_eq!(
        jars.cookie_header(
            &first,
            &wrong_plugin,
            &Url::parse("https://account.virtual.invalid/").unwrap(),
        ),
        Err(CookieJarError::BindingMismatch)
    );

    let expired = jars
        .create(binding.clone(), Duration::ZERO)
        .expect("expired session ID");
    assert_eq!(
        jars.cookie_header(
            &expired,
            &binding,
            &Url::parse("https://account.virtual.invalid/").unwrap(),
        ),
        Err(CookieJarError::Expired)
    );
}

#[test]
fn captures_distinct_set_cookie_fields_and_applies_host_path_secure_and_deletion_rules() {
    let jars = TemporaryCookieJars::new();
    let binding = CookieJarBinding::login(
        plugin_id(),
        "virtual",
        scope(),
        origins(&[
            "https://account.virtual.invalid",
            "http://account.virtual.invalid",
        ]),
    )
    .unwrap();
    let session = jars
        .create(binding.clone(), Duration::from_secs(60))
        .unwrap();
    let response_url = Url::parse("https://account.virtual.invalid/auth/login").unwrap();
    let mut headers = HeaderMap::new();
    headers.append(
        header::SET_COOKIE,
        HeaderValue::from_str(&format!("root={COOKIE_CANARY}; Path=/; Secure; HttpOnly")).unwrap(),
    );
    headers.append(
        header::SET_COOKIE,
        HeaderValue::from_static("nested=second; Path=/auth; Secure"),
    );

    jars.capture_response(&session, &binding, &response_url, &headers)
        .unwrap();

    assert_eq!(
        header_text(
            jars.cookie_header(
                &session,
                &binding,
                &Url::parse("https://account.virtual.invalid/auth/check").unwrap(),
            )
            .unwrap(),
        ),
        format!("nested=second; root={COOKIE_CANARY}")
    );
    assert_eq!(
        header_text(
            jars.cookie_header(
                &session,
                &binding,
                &Url::parse("https://account.virtual.invalid/public").unwrap(),
            )
            .unwrap(),
        ),
        format!("root={COOKIE_CANARY}")
    );
    assert!(jars
        .cookie_header(
            &session,
            &binding,
            &Url::parse("http://account.virtual.invalid/auth/check").unwrap(),
        )
        .unwrap()
        .is_none());

    let mut deletion = HeaderMap::new();
    deletion.append(
        header::SET_COOKIE,
        HeaderValue::from_static("nested=gone; Path=/auth; Max-Age=0"),
    );
    jars.capture_response(&session, &binding, &response_url, &deletion)
        .unwrap();
    assert_eq!(
        header_text(
            jars.cookie_header(
                &session,
                &binding,
                &Url::parse("https://account.virtual.invalid/auth/check").unwrap(),
            )
            .unwrap(),
        ),
        format!("root={COOKIE_CANARY}")
    );

    let mut empty_value_deletion = HeaderMap::new();
    empty_value_deletion.append(
        header::SET_COOKIE,
        HeaderValue::from_static("root=; Path=/; Max-Age=0"),
    );
    jars.capture_response(&session, &binding, &response_url, &empty_value_deletion)
        .unwrap();
    assert!(jars
        .cookie_header(
            &session,
            &binding,
            &Url::parse("https://account.virtual.invalid/").unwrap(),
        )
        .unwrap()
        .is_none());
}

#[test]
fn rejects_public_suffix_unrelated_and_malformed_cookies_atomically() {
    let jars = TemporaryCookieJars::new();
    let binding = login_binding();
    let session = jars
        .create(binding.clone(), Duration::from_secs(60))
        .unwrap();
    let response_url = Url::parse("https://account.virtual.invalid/login").unwrap();
    let mut baseline = HeaderMap::new();
    baseline.append(
        header::SET_COOKIE,
        HeaderValue::from_static("stable=kept; Path=/"),
    );
    jars.capture_response(&session, &binding, &response_url, &baseline)
        .unwrap();

    for rejected in [
        "forbidden=value; Domain=invalid; Path=/",
        "forbidden=value; Domain=other.virtual.invalid; Path=/",
        "missing-value; Path=/",
    ] {
        let mut fields = HeaderMap::new();
        fields.append(
            header::SET_COOKIE,
            HeaderValue::from_static("rolled-back=must-not-stick; Path=/"),
        );
        fields.append(header::SET_COOKIE, HeaderValue::from_str(rejected).unwrap());
        assert_eq!(
            jars.capture_response(&session, &binding, &response_url, &fields),
            Err(CookieJarError::InvalidCookie)
        );
        let emitted = header_text(
            jars.cookie_header(&session, &binding, &response_url)
                .unwrap(),
        );
        assert_eq!(emitted, "stable=kept");
        assert!(!emitted.contains("rolled-back"));
    }

    let mut non_utf8 = HeaderMap::new();
    non_utf8.append(
        header::SET_COOKIE,
        HeaderValue::from_bytes(b"bad=\xff; Path=/").unwrap(),
    );
    assert_eq!(
        jars.capture_response(&session, &binding, &response_url, &non_utf8),
        Err(CookieJarError::InvalidCookie)
    );
}

#[test]
fn promotion_normalizes_origins_and_never_exposes_cookie_values_through_debug() {
    let jars = TemporaryCookieJars::new();
    let binding = login_binding();
    let session = jars
        .create(binding.clone(), Duration::from_secs(60))
        .unwrap();
    let response_url = Url::parse("https://account.virtual.invalid/login").unwrap();
    let mut headers = HeaderMap::new();
    headers.append(
        header::SET_COOKIE,
        HeaderValue::from_str(&format!("session={COOKIE_CANARY}; Path=/; Secure")).unwrap(),
    );
    jars.capture_response(&session, &binding, &response_url, &headers)
        .unwrap();

    let snapshot = jars
        .promotion_snapshot(
            &session,
            &binding,
            &origins(&[
                "HTTPS://ACCOUNT.VIRTUAL.INVALID:443",
                "https://account.virtual.invalid",
            ]),
        )
        .unwrap();

    assert_eq!(snapshot.target_origins().len(), 1);
    assert_eq!(
        snapshot.target_origins()[0].as_str(),
        "https://account.virtual.invalid"
    );
    assert_eq!(snapshot.secret().cookies().len(), 1);
    assert_eq!(
        snapshot.secret().cookies()[0].with_value(str::to_owned),
        COOKIE_CANARY
    );
    let debug = format!("{snapshot:?}");
    for sensitive in [
        COOKIE_CANARY,
        "session",
        "account.virtual.invalid",
        "https://",
        "target_origins",
    ] {
        assert!(!debug.contains(sensitive), "debug leaked {sensitive}");
    }
    assert!(debug.contains("[REDACTED]"));
}

#[test]
fn promotion_rejects_refresh_sessions_and_origins_that_do_not_cover_cookie_hosts() {
    let jars = TemporaryCookieJars::new();
    let refresh = CookieJarBinding::refresh(
        plugin_id(),
        "virtual",
        scope(),
        origins(&["https://account.virtual.invalid"]),
        audiodown_domain::credential::CredentialId::parse("1140dabc-6207-4ee0-9100-042d5f749b5e")
            .unwrap(),
        7,
    )
    .unwrap();
    let refresh_id = jars
        .create(refresh.clone(), Duration::from_secs(60))
        .unwrap();
    assert!(matches!(
        jars.promotion_snapshot(
            &refresh_id,
            &refresh,
            &origins(&["https://account.virtual.invalid"]),
        ),
        Err(CookieJarError::PurposeMismatch)
    ));

    let login = login_binding();
    let login_id = jars.create(login.clone(), Duration::from_secs(60)).unwrap();
    let mut headers = HeaderMap::new();
    headers.append(
        header::SET_COOKIE,
        HeaderValue::from_static("session=value; Path=/"),
    );
    jars.capture_response(
        &login_id,
        &login,
        &Url::parse("https://account.virtual.invalid/").unwrap(),
        &headers,
    )
    .unwrap();
    assert!(matches!(
        jars.promotion_snapshot(
            &login_id,
            &login,
            &origins(&["https://media.virtual.invalid"]),
        ),
        Err(CookieJarError::OriginDenied)
    ));
}

#[test]
fn promotion_narrows_domain_cookies_to_each_selected_exact_origin() {
    let jars = TemporaryCookieJars::new();
    let binding = CookieJarBinding::login(
        plugin_id(),
        "virtual",
        scope(),
        origins(&[
            "https://account.virtual.invalid",
            "https://media.virtual.invalid",
        ]),
    )
    .unwrap();
    let session = jars
        .create(binding.clone(), Duration::from_secs(60))
        .unwrap();
    let mut headers = HeaderMap::new();
    headers.append(
        header::SET_COOKIE,
        HeaderValue::from_static("shared=value; Domain=virtual.invalid; Path=/; Secure"),
    );
    jars.capture_response(
        &session,
        &binding,
        &Url::parse("https://account.virtual.invalid/login").unwrap(),
        &headers,
    )
    .unwrap();

    let snapshot = jars
        .promotion_snapshot(
            &session,
            &binding,
            &origins(&[
                "https://account.virtual.invalid",
                "https://media.virtual.invalid",
            ]),
        )
        .unwrap();
    let hosts = snapshot
        .secret()
        .cookies()
        .iter()
        .map(|cookie| cookie.host())
        .collect::<Vec<_>>();
    assert_eq!(hosts, ["account.virtual.invalid", "media.virtual.invalid"]);
}

#[test]
fn honors_default_path_boundaries_and_max_age_precedence() {
    let jars = TemporaryCookieJars::new();
    let binding = login_binding();
    let session = jars
        .create(binding.clone(), Duration::from_secs(60))
        .unwrap();
    let response_url = Url::parse("https://account.virtual.invalid/auth/login").unwrap();
    let mut headers = HeaderMap::new();
    headers.append(
        header::SET_COOKIE,
        HeaderValue::from_static(
            "defaulted=value; Secure; Max-Age=60; Expires=Thu, 01 Jan 1970 00:00:00 GMT",
        ),
    );
    jars.capture_response(&session, &binding, &response_url, &headers)
        .unwrap();

    assert_eq!(
        header_text(
            jars.cookie_header(
                &session,
                &binding,
                &Url::parse("https://account.virtual.invalid/auth/check").unwrap(),
            )
            .unwrap(),
        ),
        "defaulted=value"
    );
    assert!(jars
        .cookie_header(
            &session,
            &binding,
            &Url::parse("https://account.virtual.invalid/authentication").unwrap(),
        )
        .unwrap()
        .is_none());

    let mut deletion = HeaderMap::new();
    deletion.append(
        header::SET_COOKIE,
        HeaderValue::from_static(
            "defaulted=still-present; Path=/auth; Max-Age=0; Expires=Fri, 01 Jan 2100 00:00:00 GMT",
        ),
    );
    jars.capture_response(&session, &binding, &response_url, &deletion)
        .unwrap();
    assert!(jars
        .cookie_header(
            &session,
            &binding,
            &Url::parse("https://account.virtual.invalid/auth/check").unwrap(),
        )
        .unwrap()
        .is_none());
}

#[test]
fn accepts_parser_valid_empty_and_quoted_cookie_values() {
    let jars = TemporaryCookieJars::new();
    let binding = login_binding();
    let session = jars
        .create(binding.clone(), Duration::from_secs(60))
        .unwrap();
    let url = Url::parse("https://account.virtual.invalid/").unwrap();
    let mut headers = HeaderMap::new();
    headers.append(
        header::SET_COOKIE,
        HeaderValue::from_static("empty=; Path=/; Secure"),
    );
    headers.append(
        header::SET_COOKIE,
        HeaderValue::from_static("quoted=\"two words\"; Path=/; Secure"),
    );

    jars.capture_response(&session, &binding, &url, &headers)
        .unwrap();
    assert_eq!(
        header_text(jars.cookie_header(&session, &binding, &url).unwrap()),
        "empty=; quoted=\"two words\""
    );
}

fn login_binding() -> CookieJarBinding {
    CookieJarBinding::login(
        plugin_id(),
        "virtual",
        scope(),
        origins(&["https://account.virtual.invalid"]),
    )
    .unwrap()
}

fn plugin_id() -> PluginId {
    PluginId::parse("com.example.virtual.credential").unwrap()
}

fn scope() -> CredentialScope {
    CredentialScope::parse("virtual.web").unwrap()
}

fn origins(values: &[&str]) -> Vec<CredentialTargetOrigin> {
    values
        .iter()
        .map(|value| CredentialTargetOrigin::parse(value).unwrap())
        .collect()
}

fn header_text(value: Option<HeaderValue>) -> String {
    value
        .expect("Cookie header")
        .to_str()
        .expect("ASCII Cookie header")
        .to_string()
}
