use audiodown_credential_vault::{
    decrypt, encrypt, CryptoError, EncryptedEnvelope, EncryptionContext, MasterKey,
    ALGORITHM_VERSION, MAX_PLAINTEXT_BYTES,
};
use audiodown_domain::credential::{CredentialId, CredentialScope};
use secrecy::{ExposeSecret, Secret, SecretVec};

const RECORD_ONE: &str = "8d86182f-95f7-44d8-a75c-b9d1ec2c18ad";
const RECORD_TWO: &str = "2a8a66ff-8b72-41f4-8ca2-3d178d4cb318";

fn master_key(fill: u8) -> MasterKey {
    MasterKey::from_secret(Secret::new([fill; 32]))
}

fn encryption_context(record_id: &str, scope: &str, key_version: u32) -> EncryptionContext {
    EncryptionContext::new(
        CredentialId::parse(record_id).unwrap(),
        CredentialScope::parse(scope).unwrap(),
        key_version,
    )
}

fn secret(value: &[u8]) -> SecretVec<u8> {
    SecretVec::new(value.to_vec())
}

#[test]
fn round_trips_secrets_with_fresh_96_bit_nonces() {
    let key = master_key(0x11);
    let context = encryption_context(RECORD_ONE, "virtual.web", 1);
    let plaintext = secret(b"synthetic-cookie=session-canary-one");

    let first = encrypt(&key, &context, &plaintext).unwrap();
    let second = encrypt(&key, &context, &plaintext).unwrap();

    assert_eq!(first.nonce().len(), 12);
    assert_ne!(first.nonce(), second.nonce());
    assert_eq!(first.algorithm_version(), ALGORITHM_VERSION);
    assert_eq!(first.key_version(), 1);
    assert!(!first
        .ciphertext()
        .windows(plaintext.expose_secret().len())
        .any(|window| window == plaintext.expose_secret()));

    let decrypted = decrypt(&key, &context, &first).unwrap();
    assert_eq!(decrypted.expose_secret(), plaintext.expose_secret());
}

#[test]
fn rejects_wrong_keys_and_tampered_ciphertext_nonce_or_associated_data() {
    let key = master_key(0x22);
    let context = encryption_context(RECORD_ONE, "virtual.web", 7);
    let plaintext = secret(b"synthetic-token-canary-two");
    let envelope = encrypt(&key, &context, &plaintext).unwrap();

    assert_eq!(
        decrypt(&master_key(0x23), &context, &envelope)
            .err()
            .unwrap(),
        CryptoError::AuthenticationFailed
    );

    let mut tampered_ciphertext = envelope.ciphertext().to_vec();
    tampered_ciphertext[0] ^= 0x80;
    let tampered_ciphertext = EncryptedEnvelope::from_parts(
        envelope.algorithm_version(),
        envelope.key_version(),
        *envelope.nonce(),
        tampered_ciphertext,
    );
    assert_eq!(
        decrypt(&key, &context, &tampered_ciphertext).err().unwrap(),
        CryptoError::AuthenticationFailed
    );

    let mut tampered_nonce = *envelope.nonce();
    tampered_nonce[0] ^= 0x40;
    let tampered_nonce = EncryptedEnvelope::from_parts(
        envelope.algorithm_version(),
        envelope.key_version(),
        tampered_nonce,
        envelope.ciphertext().to_vec(),
    );
    assert_eq!(
        decrypt(&key, &context, &tampered_nonce).err().unwrap(),
        CryptoError::AuthenticationFailed
    );

    for changed_context in [
        encryption_context(RECORD_TWO, "virtual.web", 7),
        encryption_context(RECORD_ONE, "virtual.mobile", 7),
    ] {
        assert_eq!(
            decrypt(&key, &changed_context, &envelope).err().unwrap(),
            CryptoError::AuthenticationFailed
        );
    }
}

#[test]
fn rejects_oversized_plaintext_and_unsupported_versions() {
    let key = master_key(0x33);
    let context = encryption_context(RECORD_ONE, "virtual.web", 1);

    assert_eq!(
        encrypt(
            &key,
            &context,
            &secret(&vec![0x41; MAX_PLAINTEXT_BYTES + 1]),
        )
        .unwrap_err(),
        CryptoError::PlaintextTooLarge
    );
    let maximum = secret(&vec![0x42; MAX_PLAINTEXT_BYTES]);
    let maximum_envelope = encrypt(&key, &context, &maximum).unwrap();
    assert_eq!(
        decrypt(&key, &context, &maximum_envelope)
            .unwrap()
            .expose_secret(),
        maximum.expose_secret()
    );

    let empty = secret(b"");
    let empty_envelope = encrypt(&key, &context, &empty).unwrap();
    assert!(decrypt(&key, &context, &empty_envelope)
        .unwrap()
        .expose_secret()
        .is_empty());

    let unsupported = EncryptionContext::from_parts(
        context.record_id(),
        context.scope().clone(),
        ALGORITHM_VERSION + 1,
        1,
    );
    assert_eq!(
        encrypt(&key, &unsupported, &secret(b"synthetic")).unwrap_err(),
        CryptoError::UnsupportedAlgorithmVersion
    );

    let invalid_key_version =
        EncryptionContext::from_parts(context.record_id(), context.scope().clone(), 1, 0);
    assert_eq!(
        encrypt(&key, &invalid_key_version, &secret(b"synthetic")).unwrap_err(),
        CryptoError::InvalidKeyVersion
    );

    let short = EncryptedEnvelope::from_parts(ALGORITHM_VERSION, 1, [0x11; 12], vec![0_u8; 15]);
    assert_eq!(
        decrypt(&key, &context, &short).err().unwrap(),
        CryptoError::InvalidEnvelope
    );
    let oversized = EncryptedEnvelope::from_parts(
        ALGORITHM_VERSION,
        1,
        [0x11; 12],
        vec![0_u8; MAX_PLAINTEXT_BYTES + 17],
    );
    assert_eq!(
        decrypt(&key, &context, &oversized).err().unwrap(),
        CryptoError::InvalidEnvelope
    );
    let zero_key_version =
        EncryptedEnvelope::from_parts(ALGORITHM_VERSION, 0, [0x11; 12], vec![0_u8; 16]);
    assert_eq!(
        decrypt(&key, &context, &zero_key_version).err().unwrap(),
        CryptoError::InvalidKeyVersion
    );
}

#[test]
fn rejects_cross_record_scope_algorithm_and_key_version_substitution() {
    let key = master_key(0x44);
    let context = encryption_context(RECORD_ONE, "virtual.web", 9);
    let envelope = encrypt(&key, &context, &secret(b"substitution-canary")).unwrap();

    let contexts = [
        encryption_context(RECORD_TWO, "virtual.web", 9),
        encryption_context(RECORD_ONE, "virtual.mobile", 9),
        EncryptionContext::from_parts(
            context.record_id(),
            context.scope().clone(),
            ALGORITHM_VERSION + 1,
            9,
        ),
        encryption_context(RECORD_ONE, "virtual.web", 10),
    ];
    for substituted in contexts {
        assert!(decrypt(&key, &substituted, &envelope).is_err());
    }

    let algorithm_substitution = EncryptedEnvelope::from_parts(
        ALGORITHM_VERSION + 1,
        envelope.key_version(),
        *envelope.nonce(),
        envelope.ciphertext().to_vec(),
    );
    assert_eq!(
        decrypt(&key, &context, &algorithm_substitution)
            .err()
            .unwrap(),
        CryptoError::UnsupportedAlgorithmVersion
    );

    let key_version_substitution = EncryptedEnvelope::from_parts(
        envelope.algorithm_version(),
        envelope.key_version() + 1,
        *envelope.nonce(),
        envelope.ciphertext().to_vec(),
    );
    assert_eq!(
        decrypt(&key, &context, &key_version_substitution)
            .err()
            .unwrap(),
        CryptoError::EnvelopeMetadataMismatch
    );

    let substituted_context =
        encryption_context(RECORD_ONE, "virtual.web", envelope.key_version() + 1);
    assert_eq!(
        decrypt(&key, &substituted_context, &key_version_substitution)
            .err()
            .unwrap(),
        CryptoError::AuthenticationFailed
    );
}

#[test]
fn debug_and_errors_never_expose_key_plaintext_nonce_or_ciphertext() {
    let plaintext_canary = "debug-plaintext-canary";
    let key = MasterKey::from_secret(Secret::new([0x55; 32]));
    let context = encryption_context(RECORD_ONE, "virtual.web", 3);
    let envelope = encrypt(&key, &context, &secret(plaintext_canary.as_bytes())).unwrap();
    let error = decrypt(&master_key(0x56), &context, &envelope)
        .err()
        .unwrap();

    let rendered = format!("{key:?}\n{envelope:?}\n{error:?}\n{error}");
    assert!(!rendered.contains(plaintext_canary));
    assert!(!rendered.contains(&format!("{:?}", vec![0x55; 32])));
    assert!(!rendered.contains(&format!("{:?}", envelope.nonce())));
    assert!(!rendered.contains(&format!("{:?}", envelope.ciphertext())));
    assert!(rendered.contains("[REDACTED]"));
}
