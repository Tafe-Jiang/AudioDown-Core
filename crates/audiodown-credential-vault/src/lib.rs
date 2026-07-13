mod crypto;
mod key_store;
mod secret;
mod service;

pub use crypto::{
    decrypt, encrypt, CryptoError, EncryptedEnvelope, EncryptionContext, MasterKey,
    ALGORITHM_VERSION, MAX_PLAINTEXT_BYTES,
};
pub use key_store::{load_or_create_master_key, CredentialKeyStoreError};
pub use secret::{
    CookieCredentialSecret, CookieSecretRecord, CredentialSecretGuard, SecretPayloadError,
    TokenCredentialSecret, SECRET_PAYLOAD_VERSION,
};
pub use service::{
    CredentialCreateRequest, CredentialMetadata, CredentialRepository, CredentialRepositoryError,
    CredentialUpdateRequest, CredentialVault, MetadataPort, SecretsPort, StoredCredential,
    TrustedPort, VaultError,
};
