use std::fmt;

use aes_gcm::{
    aead::{array::Array, Aead, KeyInit, Payload},
    Aes256Gcm,
};
use audiodown_domain::credential::{CredentialId, CredentialScope};
use rand_core::{CryptoRng, OsRng, RngCore};
use secrecy::{ExposeSecret, Secret, SecretVec};
use thiserror::Error;

pub const ALGORITHM_VERSION: u16 = 1;
pub const MAX_PLAINTEXT_BYTES: usize = 64 * 1024;

const NONCE_BYTES: usize = 12;
const AUTHENTICATION_TAG_BYTES: usize = 16;
const ASSOCIATED_DATA_PREFIX: &[u8] = b"audiodown-credential-envelope\0";

pub struct MasterKey(Secret<[u8; 32]>);

impl MasterKey {
    pub fn from_secret(secret: Secret<[u8; 32]>) -> Self {
        Self(secret)
    }
}

impl fmt::Debug for MasterKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("MasterKey")
            .field(&"[REDACTED]")
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncryptionContext {
    record_id: CredentialId,
    scope: CredentialScope,
    algorithm_version: u16,
    key_version: u32,
}

impl EncryptionContext {
    pub fn new(record_id: CredentialId, scope: CredentialScope, key_version: u32) -> Self {
        Self::from_parts(record_id, scope, ALGORITHM_VERSION, key_version)
    }

    pub fn from_parts(
        record_id: CredentialId,
        scope: CredentialScope,
        algorithm_version: u16,
        key_version: u32,
    ) -> Self {
        Self {
            record_id,
            scope,
            algorithm_version,
            key_version,
        }
    }

    pub fn record_id(&self) -> CredentialId {
        self.record_id
    }

    pub fn scope(&self) -> &CredentialScope {
        &self.scope
    }

    pub fn algorithm_version(&self) -> u16 {
        self.algorithm_version
    }

    pub fn key_version(&self) -> u32 {
        self.key_version
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct EncryptedEnvelope {
    algorithm_version: u16,
    key_version: u32,
    nonce: [u8; NONCE_BYTES],
    ciphertext: Vec<u8>,
}

impl EncryptedEnvelope {
    pub fn from_parts(
        algorithm_version: u16,
        key_version: u32,
        nonce: [u8; NONCE_BYTES],
        ciphertext: Vec<u8>,
    ) -> Self {
        Self {
            algorithm_version,
            key_version,
            nonce,
            ciphertext,
        }
    }

    pub fn algorithm_version(&self) -> u16 {
        self.algorithm_version
    }

    pub fn key_version(&self) -> u32 {
        self.key_version
    }

    pub fn nonce(&self) -> &[u8; NONCE_BYTES] {
        &self.nonce
    }

    pub fn ciphertext(&self) -> &[u8] {
        &self.ciphertext
    }
}

impl fmt::Debug for EncryptedEnvelope {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EncryptedEnvelope")
            .field("algorithm_version", &self.algorithm_version)
            .field("key_version", &self.key_version)
            .field("nonce", &"[REDACTED]")
            .field(
                "ciphertext",
                &format!("[REDACTED; {} bytes]", self.ciphertext.len()),
            )
            .finish()
    }
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum CryptoError {
    #[error("credential plaintext exceeds the allowed size")]
    PlaintextTooLarge,
    #[error("credential envelope uses an unsupported algorithm version")]
    UnsupportedAlgorithmVersion,
    #[error("credential envelope uses an invalid key version")]
    InvalidKeyVersion,
    #[error("credential envelope metadata does not match its context")]
    EnvelopeMetadataMismatch,
    #[error("credential envelope is malformed")]
    InvalidEnvelope,
    #[error("credential encryption failed")]
    EncryptionFailed,
    #[error("operating system randomness is unavailable")]
    RandomnessUnavailable,
    #[error("credential authentication failed")]
    AuthenticationFailed,
}

pub fn encrypt(
    key: &MasterKey,
    context: &EncryptionContext,
    plaintext: &SecretVec<u8>,
) -> Result<EncryptedEnvelope, CryptoError> {
    validate_context(context)?;
    validate_plaintext(plaintext)?;
    let nonce = nonce_from_rng(&mut OsRng)?;
    encrypt_with_nonce(key, context, plaintext, nonce)
}

fn encrypt_with_nonce(
    key: &MasterKey,
    context: &EncryptionContext,
    plaintext: &SecretVec<u8>,
    nonce: [u8; NONCE_BYTES],
) -> Result<EncryptedEnvelope, CryptoError> {
    validate_context(context)?;
    validate_plaintext(plaintext)?;

    let cipher = Aes256Gcm::new_from_slice(key.0.expose_secret())
        .map_err(|_| CryptoError::EncryptionFailed)?;
    let associated_data = associated_data(context);
    let ciphertext = cipher
        .encrypt(
            &Array(nonce),
            Payload {
                msg: plaintext.expose_secret(),
                aad: &associated_data,
            },
        )
        .map_err(|_| CryptoError::EncryptionFailed)?;

    Ok(EncryptedEnvelope {
        algorithm_version: context.algorithm_version,
        key_version: context.key_version,
        nonce,
        ciphertext,
    })
}

pub fn decrypt(
    key: &MasterKey,
    context: &EncryptionContext,
    envelope: &EncryptedEnvelope,
) -> Result<SecretVec<u8>, CryptoError> {
    validate_context(context)?;
    validate_envelope(context, envelope)?;

    let cipher = Aes256Gcm::new_from_slice(key.0.expose_secret())
        .map_err(|_| CryptoError::AuthenticationFailed)?;
    let associated_data = associated_data(context);
    let plaintext = cipher
        .decrypt(
            &Array(envelope.nonce),
            Payload {
                msg: &envelope.ciphertext,
                aad: &associated_data,
            },
        )
        .map_err(|_| CryptoError::AuthenticationFailed)?;
    Ok(SecretVec::new(plaintext))
}

fn nonce_from_rng(rng: &mut (impl CryptoRng + RngCore)) -> Result<[u8; NONCE_BYTES], CryptoError> {
    let mut nonce = [0_u8; NONCE_BYTES];
    rng.try_fill_bytes(&mut nonce)
        .map_err(|_| CryptoError::RandomnessUnavailable)?;
    Ok(nonce)
}

fn validate_context(context: &EncryptionContext) -> Result<(), CryptoError> {
    if context.algorithm_version != ALGORITHM_VERSION {
        return Err(CryptoError::UnsupportedAlgorithmVersion);
    }
    if context.key_version == 0 {
        return Err(CryptoError::InvalidKeyVersion);
    }
    Ok(())
}

fn validate_plaintext(plaintext: &SecretVec<u8>) -> Result<(), CryptoError> {
    if plaintext.expose_secret().len() > MAX_PLAINTEXT_BYTES {
        Err(CryptoError::PlaintextTooLarge)
    } else {
        Ok(())
    }
}

fn validate_envelope(
    context: &EncryptionContext,
    envelope: &EncryptedEnvelope,
) -> Result<(), CryptoError> {
    if envelope.algorithm_version != ALGORITHM_VERSION {
        return Err(CryptoError::UnsupportedAlgorithmVersion);
    }
    if envelope.key_version == 0 {
        return Err(CryptoError::InvalidKeyVersion);
    }
    if envelope.algorithm_version != context.algorithm_version
        || envelope.key_version != context.key_version
    {
        return Err(CryptoError::EnvelopeMetadataMismatch);
    }
    if envelope.ciphertext.len() < AUTHENTICATION_TAG_BYTES
        || envelope.ciphertext.len() > MAX_PLAINTEXT_BYTES + AUTHENTICATION_TAG_BYTES
    {
        return Err(CryptoError::InvalidEnvelope);
    }
    Ok(())
}

fn associated_data(context: &EncryptionContext) -> Vec<u8> {
    let scope = context.scope.as_str().as_bytes();
    let mut output = Vec::with_capacity(
        ASSOCIATED_DATA_PREFIX.len()
            + size_of::<u16>()
            + size_of::<u32>()
            + 16
            + size_of::<u16>()
            + scope.len(),
    );
    output.extend_from_slice(ASSOCIATED_DATA_PREFIX);
    output.extend_from_slice(&context.algorithm_version.to_be_bytes());
    output.extend_from_slice(&context.key_version.to_be_bytes());
    output.extend_from_slice(context.record_id.as_uuid().as_bytes());
    output.extend_from_slice(&(scope.len() as u16).to_be_bytes());
    output.extend_from_slice(scope);
    output
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;

    use rand_core::{CryptoRng, Error as RandError, RngCore};
    use secrecy::{ExposeSecret, Secret};
    use zeroize::ZeroizeOnDrop;

    use super::*;

    struct FailingRng;

    impl RngCore for FailingRng {
        fn next_u32(&mut self) -> u32 {
            0
        }

        fn next_u64(&mut self) -> u64 {
            0
        }

        fn fill_bytes(&mut self, _dest: &mut [u8]) {
            panic!("fill_bytes must not be used")
        }

        fn try_fill_bytes(&mut self, _dest: &mut [u8]) -> Result<(), RandError> {
            Err(RandError::from(
                NonZeroU32::new(RandError::CUSTOM_START).unwrap(),
            ))
        }
    }

    impl CryptoRng for FailingRng {}

    #[test]
    fn derived_cipher_states_are_zeroized_on_drop() {
        fn assert_zeroize_on_drop<T: ZeroizeOnDrop>() {}

        assert_zeroize_on_drop::<aes::Aes256>();
        assert!(std::mem::needs_drop::<Aes256Gcm>());
        assert!(std::mem::needs_drop::<ghash::GHash>());
        assert!(std::mem::needs_drop::<polyval::Polyval>());
    }

    #[test]
    fn randomness_failure_returns_a_stable_error() {
        assert_eq!(
            nonce_from_rng(&mut FailingRng).unwrap_err(),
            CryptoError::RandomnessUnavailable
        );
    }

    #[test]
    fn canonical_aad_and_ciphertext_vector_is_stable() {
        let key = MasterKey::from_secret(Secret::new([0x66; 32]));
        let context = EncryptionContext::new(
            CredentialId::parse("8d86182f-95f7-44d8-a75c-b9d1ec2c18ad").unwrap(),
            CredentialScope::parse("virtual.web").unwrap(),
            3,
        );
        let nonce = [0x77; 12];
        let plaintext = SecretVec::new(b"canonical-aad-vector".to_vec());
        let envelope = encrypt_with_nonce(&key, &context, &plaintext, nonce).unwrap();

        assert_eq!(
            envelope.ciphertext(),
            &[
                0xbe, 0x6b, 0xc5, 0x24, 0xa9, 0xce, 0xf4, 0x74, 0xb4, 0xcb, 0xa4, 0xac, 0xfb, 0x72,
                0xee, 0x67, 0x6a, 0x0a, 0x05, 0x63, 0x14, 0xb1, 0x86, 0xd0, 0x73, 0xaa, 0x70, 0xb1,
                0xbc, 0x4e, 0xe0, 0xc9, 0x66, 0x1e, 0x39, 0xb5,
            ]
        );
        assert_eq!(
            decrypt(&key, &context, &envelope).unwrap().expose_secret(),
            plaintext.expose_secret()
        );
    }
}
