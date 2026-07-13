use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use audiodown_credential_vault::{
    CookieCredentialSecret, CookieSecretRecord, CredentialCreateRequest, CredentialRepository,
    CredentialRepositoryError, CredentialUpdateRequest, CredentialVault, MasterKey,
    StoredCredential, TokenCredentialSecret, VaultError,
};
use audiodown_domain::{
    credential::{CredentialId, CredentialKind, CredentialScope, CredentialStatus},
    plugin::PluginId,
};
use audiodown_plugin_api::manifest::CredentialTargetOrigin;
use chrono::{Duration, Utc};
use secrecy::{Secret, SecretString};
use tokio::sync::Barrier;

const COOKIE_CANARY: &str = "cookie-service-canary-must-remain-secret";
const TOKEN_CANARY: &str = "token-service-canary-must-remain-secret";

#[tokio::test]
async fn trusted_writes_encrypt_cookie_and_token_while_metadata_stays_safe(
) -> Result<(), Box<dyn std::error::Error>> {
    let repository = MemoryRepository::default();
    let vault = CredentialVault::new(master_key(), repository.clone());
    let owner = PluginId::parse("com.audiodown.virtual.credential")?;

    let cookie = vault
        .trusted()
        .create_cookie(
            create_request("virtual.web", Some(owner.clone()), None),
            cookie_secret(COOKIE_CANARY, "account.virtual.invalid"),
        )
        .await?;
    let token = vault
        .trusted()
        .create_token(
            create_request("virtual.token", Some(owner), None),
            TokenCredentialSecret::bearer(SecretString::new(TOKEN_CANARY.to_string()))?,
        )
        .await?;

    assert_eq!(cookie.metadata.kind, CredentialKind::Cookie);
    assert_eq!(cookie.revision, 1);
    assert_eq!(token.metadata.kind, CredentialKind::Token);
    assert_eq!(
        cookie.metadata.target_origins[0].as_str(),
        "https://account.virtual.invalid"
    );

    let records = repository.snapshot();
    assert_eq!(records.len(), 2);
    for record in records.values() {
        let rendered = format!("{record:?}");
        assert!(!rendered.contains(COOKIE_CANARY));
        assert!(!rendered.contains(TOKEN_CANARY));
        assert!(!record
            .envelope
            .ciphertext()
            .windows(COOKIE_CANARY.len())
            .any(|window| window == COOKIE_CANARY.as_bytes()));
        assert!(!record
            .envelope
            .ciphertext()
            .windows(TOKEN_CANARY.len())
            .any(|window| window == TOKEN_CANARY.as_bytes()));
    }

    let metadata = vault.metadata().list().await?;
    let serialized = serde_json::to_string(&metadata)?;
    assert!(!serialized.contains(COOKIE_CANARY));
    assert!(!serialized.contains(TOKEN_CANARY));
    for forbidden in ["ciphertext", "nonce", "keyVersion", "authorization"] {
        assert!(
            !serialized
                .to_ascii_lowercase()
                .contains(&forbidden.to_ascii_lowercase()),
            "metadata leaked forbidden field {forbidden}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn internal_secret_guard_decrypts_values_and_rejects_expired_credentials(
) -> Result<(), Box<dyn std::error::Error>> {
    let repository = MemoryRepository::default();
    let vault = CredentialVault::new(master_key(), repository);
    let active = vault
        .trusted()
        .create_cookie(
            create_request("virtual.web", None, Some(Utc::now() + Duration::hours(1))),
            cookie_secret(COOKIE_CANARY, "account.virtual.invalid"),
        )
        .await?;

    let guard = vault.secrets().open(&active.metadata.id).await?;
    let cookie = guard.cookie().expect("cookie secret");
    assert_eq!(cookie.cookies().len(), 1);
    assert_eq!(cookie.cookies()[0].name(), "session");
    assert_eq!(cookie.cookies()[0].host(), "account.virtual.invalid");
    assert_eq!(
        cookie.cookies()[0].with_value(str::to_string),
        COOKIE_CANARY
    );
    let rendered = format!("{guard:?}\n{cookie:?}\n{:?}", cookie.cookies()[0]);
    assert!(!rendered.contains(COOKIE_CANARY));

    let expired = vault
        .trusted()
        .create_token(
            create_request(
                "virtual.expired",
                None,
                Some(Utc::now() - Duration::seconds(1)),
            ),
            TokenCredentialSecret::bearer(SecretString::new(TOKEN_CANARY.to_string()))?,
        )
        .await?;
    assert_eq!(
        vault
            .metadata()
            .get(&expired.metadata.id)
            .await?
            .unwrap()
            .status,
        CredentialStatus::Expired
    );
    assert!(matches!(
        vault.secrets().open(&expired.metadata.id).await,
        Err(VaultError::Expired)
    ));
    Ok(())
}

#[tokio::test]
async fn validates_origin_binding_and_supports_retain_and_delete(
) -> Result<(), Box<dyn std::error::Error>> {
    let repository = MemoryRepository::default();
    let vault = CredentialVault::new(master_key(), repository.clone());
    let owner = PluginId::parse("com.audiodown.virtual.owner")?;

    let error = vault
        .trusted()
        .create_cookie(
            create_request("virtual.web", Some(owner.clone()), None),
            cookie_secret(COOKIE_CANARY, "outside.virtual.invalid"),
        )
        .await
        .unwrap_err();
    assert!(matches!(error, VaultError::InvalidRequest));
    assert!(repository.snapshot().is_empty());

    let created = vault
        .trusted()
        .create_cookie(
            create_request("virtual.web", Some(owner.clone()), None),
            cookie_secret(COOKIE_CANARY, "account.virtual.invalid"),
        )
        .await?;
    assert_eq!(created.metadata.ownership.source_plugin_id(), Some(&owner));

    let retained = vault.metadata().retain(&created.metadata.id).await?;
    assert!(retained.ownership.is_retained());
    assert!(repository
        .snapshot()
        .get(&created.metadata.id)
        .unwrap()
        .source_plugin_id
        .is_none());

    vault.metadata().delete(&created.metadata.id).await?;
    assert!(vault.metadata().get(&created.metadata.id).await?.is_none());
    Ok(())
}

#[tokio::test]
async fn concurrent_updates_conflict_and_repository_failures_roll_back(
) -> Result<(), Box<dyn std::error::Error>> {
    let repository = MemoryRepository::default();
    let vault = Arc::new(CredentialVault::new(master_key(), repository.clone()));
    let created = vault
        .trusted()
        .create_cookie(
            create_request("virtual.web", None, None),
            cookie_secret(COOKIE_CANARY, "account.virtual.invalid"),
        )
        .await?;

    let barrier = Arc::new(Barrier::new(3));
    let handles = ["updated-a", "updated-b"].map(|value| {
        let vault = vault.clone();
        let barrier = barrier.clone();
        let request = update_request(created.metadata.id, created.revision);
        tokio::spawn(async move {
            barrier.wait().await;
            vault
                .trusted()
                .update_cookie(request, cookie_secret(value, "account.virtual.invalid"))
                .await
        })
    });
    barrier.wait().await;
    let [first_handle, second_handle] = handles;
    let first = first_handle.await?;
    let second = second_handle.await?;
    let success = match (first, second) {
        (Ok(result), Err(VaultError::Conflict)) | (Err(VaultError::Conflict), Ok(result)) => result,
        results => panic!("unexpected update results: {results:?}"),
    };
    assert_eq!(success.revision, 2);

    let before_failure = cookie_value(&vault.secrets().open(&created.metadata.id).await?);
    repository.fail_next(CredentialRepositoryError::Unavailable);
    let error = vault
        .trusted()
        .update_cookie(
            update_request(created.metadata.id, success.revision),
            cookie_secret("failed-update-canary", "account.virtual.invalid"),
        )
        .await
        .unwrap_err();
    assert!(matches!(error, VaultError::RepositoryUnavailable));
    let after_failure = cookie_value(&vault.secrets().open(&created.metadata.id).await?);
    assert_eq!(after_failure, before_failure);

    repository.fail_next(CredentialRepositoryError::Unavailable);
    let create_error = vault
        .trusted()
        .create_token(
            create_request("virtual.failed", None, None),
            TokenCredentialSecret::bearer(SecretString::new("failed-create-canary".to_string()))?,
        )
        .await
        .unwrap_err();
    assert!(matches!(create_error, VaultError::RepositoryUnavailable));
    assert_eq!(repository.snapshot().len(), 1);
    let rendered = format!("{error:?}\n{create_error:?}");
    assert!(!rendered.contains("failed-update-canary"));
    assert!(!rendered.contains("failed-create-canary"));
    Ok(())
}

fn master_key() -> MasterKey {
    MasterKey::from_secret(Secret::new([0x5A; 32]))
}

fn create_request(
    scope: &str,
    source_plugin_id: Option<PluginId>,
    expires_at: Option<chrono::DateTime<Utc>>,
) -> CredentialCreateRequest {
    CredentialCreateRequest {
        platform_id: "virtual".to_string(),
        scope: CredentialScope::parse(scope).unwrap(),
        source_plugin_id,
        target_origins: vec![
            CredentialTargetOrigin::parse("HTTPS://ACCOUNT.VIRTUAL.INVALID:443").unwrap(),
        ],
        account_id_hint: Some("virtual-account".to_string()),
        display_name: Some("Virtual Account".to_string()),
        expires_at,
    }
}

fn update_request(credential_id: CredentialId, expected_revision: u64) -> CredentialUpdateRequest {
    CredentialUpdateRequest {
        credential_id,
        expected_revision,
        target_origins: vec![
            CredentialTargetOrigin::parse("https://account.virtual.invalid").unwrap(),
        ],
        account_id_hint: Some("virtual-account-updated".to_string()),
        display_name: Some("Virtual Account Updated".to_string()),
        status: CredentialStatus::Active,
        safe_error_summary: None,
        expires_at: None,
        status_checked_at: Some(Utc::now()),
    }
}

fn cookie_secret(value: &str, host: &str) -> CookieCredentialSecret {
    CookieCredentialSecret::new(vec![CookieSecretRecord::new(
        "session",
        SecretString::new(value.to_string()),
        host,
        "/",
        true,
        true,
        None,
    )
    .unwrap()])
    .unwrap()
}

fn cookie_value(guard: &audiodown_credential_vault::CredentialSecretGuard) -> String {
    guard.cookie().unwrap().cookies()[0].with_value(str::to_string)
}

#[derive(Clone, Default)]
struct MemoryRepository {
    state: Arc<Mutex<MemoryState>>,
}

#[derive(Default)]
struct MemoryState {
    records: HashMap<CredentialId, StoredCredential>,
    fail_next: Option<CredentialRepositoryError>,
}

impl MemoryRepository {
    fn snapshot(&self) -> HashMap<CredentialId, StoredCredential> {
        self.state.lock().unwrap().records.clone()
    }

    fn fail_next(&self, error: CredentialRepositoryError) {
        self.state.lock().unwrap().fail_next = Some(error);
    }

    fn maybe_fail(state: &mut MemoryState) -> Result<(), CredentialRepositoryError> {
        match state.fail_next.take() {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

#[async_trait]
impl CredentialRepository for MemoryRepository {
    async fn insert(&self, record: &StoredCredential) -> Result<(), CredentialRepositoryError> {
        let mut state = self.state.lock().unwrap();
        Self::maybe_fail(&mut state)?;
        if state.records.contains_key(&record.id)
            || state
                .records
                .values()
                .any(|stored| stored.scope == record.scope)
        {
            return Err(CredentialRepositoryError::Conflict);
        }
        state.records.insert(record.id, record.clone());
        Ok(())
    }

    async fn update(
        &self,
        record: &StoredCredential,
        expected_revision: u64,
    ) -> Result<u64, CredentialRepositoryError> {
        let mut state = self.state.lock().unwrap();
        Self::maybe_fail(&mut state)?;
        let stored = state
            .records
            .get_mut(&record.id)
            .ok_or(CredentialRepositoryError::NotFound)?;
        if stored.revision != expected_revision {
            return Err(CredentialRepositoryError::Conflict);
        }
        let next_revision = expected_revision + 1;
        let mut replacement = record.clone();
        replacement.revision = next_revision;
        *stored = replacement;
        Ok(next_revision)
    }

    async fn get(
        &self,
        credential_id: &CredentialId,
    ) -> Result<Option<StoredCredential>, CredentialRepositoryError> {
        Ok(self
            .state
            .lock()
            .unwrap()
            .records
            .get(credential_id)
            .cloned())
    }

    async fn list(&self) -> Result<Vec<StoredCredential>, CredentialRepositoryError> {
        Ok(self
            .state
            .lock()
            .unwrap()
            .records
            .values()
            .cloned()
            .collect())
    }

    async fn delete(&self, credential_id: &CredentialId) -> Result<(), CredentialRepositoryError> {
        let mut state = self.state.lock().unwrap();
        Self::maybe_fail(&mut state)?;
        state
            .records
            .remove(credential_id)
            .map(|_| ())
            .ok_or(CredentialRepositoryError::NotFound)
    }

    async fn clear_source_plugin(
        &self,
        credential_id: &CredentialId,
    ) -> Result<(), CredentialRepositoryError> {
        let mut state = self.state.lock().unwrap();
        Self::maybe_fail(&mut state)?;
        let record = state
            .records
            .get_mut(credential_id)
            .ok_or(CredentialRepositoryError::NotFound)?;
        record.source_plugin_id = None;
        record.revision += 1;
        record.updated_at = Utc::now();
        Ok(())
    }
}
