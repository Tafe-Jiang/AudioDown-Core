use audiodown_server::{config::Config, state::DevelopmentConfig};
use secrecy::SecretString;
use std::path::Path;

#[test]
fn development_tokens_are_secret_strings_with_redacted_debug_output() {
    fn assert_secret(_: &Option<SecretString>) {}

    let development = DevelopmentConfig {
        enabled: true,
        token: Some(SecretString::from("do-not-log-me".to_string())),
    };
    assert_secret(&development.token);
    let debug = format!("{development:?}");
    assert!(!debug.contains("do-not-log-me"));

    let config = Config::for_test_with_dev_token("do-not-log-me");
    assert_secret(&config.dev_token);
    let debug = format!("{config:?}");
    assert!(!debug.contains("do-not-log-me"));
}

#[test]
fn development_configuration_serializes_only_the_mode_flag() {
    let development = DevelopmentConfig {
        enabled: true,
        token: Some(SecretString::from("do-not-serialize".to_string())),
    };

    let value = development.public_view();
    assert_eq!(value, serde_json::json!({"developmentMode": true}));
    assert!(!value.to_string().contains("do-not-serialize"));
}

#[test]
fn credential_paths_are_derived_from_the_data_directory() {
    let config = Config::for_test_with_dev_token("hidden");

    assert_eq!(
        config.credentials_dir(),
        Path::new("/tmp/audiodown-test/credentials")
    );
    assert_eq!(
        config.master_key_path(),
        Path::new("/tmp/audiodown-test/credentials/master.key")
    );
}
