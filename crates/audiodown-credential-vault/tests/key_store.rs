#![cfg(unix)]

use std::{
    fs,
    os::unix::fs::{symlink, PermissionsExt},
    path::{Path, PathBuf},
    sync::{Arc, Barrier},
    thread,
};

use audiodown_credential_vault::{
    decrypt, encrypt, load_or_create_master_key, CredentialKeyStoreError, EncryptionContext,
};
use audiodown_domain::credential::{CredentialId, CredentialScope};
use rustix::{
    fs::{chown, Uid},
    process::geteuid,
};
use secrecy::{ExposeSecret, SecretVec};
use tempfile::{tempdir, TempDir};

#[cfg(target_os = "macos")]
use exacl::{setfacl, AclEntry, Perm};

fn isolated_key_path() -> (TempDir, PathBuf) {
    let temporary = tempdir().unwrap();
    let repository = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    assert!(!temporary.path().starts_with(repository));

    let data_dir = temporary.path().join("data");
    fs::create_dir(&data_dir).unwrap();
    fs::set_permissions(&data_dir, fs::Permissions::from_mode(0o700)).unwrap();
    let key_path = data_dir.join("credentials").join("master.key");
    (temporary, key_path)
}

fn context() -> EncryptionContext {
    EncryptionContext::new(
        CredentialId::parse("8d86182f-95f7-44d8-a75c-b9d1ec2c18ad").unwrap(),
        CredentialScope::parse("virtual.web").unwrap(),
        1,
    )
}

fn assert_same_key(
    first: &audiodown_credential_vault::MasterKey,
    second: &audiodown_credential_vault::MasterKey,
) {
    let plaintext = SecretVec::new(b"ephemeral-key-store-canary".to_vec());
    let envelope = encrypt(first, &context(), &plaintext).unwrap();
    assert_eq!(
        decrypt(second, &context(), &envelope)
            .unwrap()
            .expose_secret(),
        plaintext.expose_secret()
    );
}

#[test]
fn creates_atomically_with_restrictive_permissions_and_reuses_the_key() {
    let (_temporary, key_path) = isolated_key_path();
    let credentials_dir = key_path.parent().unwrap();

    let first = load_or_create_master_key(&key_path).unwrap();
    let second = load_or_create_master_key(&key_path).unwrap();

    assert_same_key(&first, &second);
    assert_eq!(
        fs::symlink_metadata(credentials_dir)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert_eq!(
        fs::symlink_metadata(&key_path)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert_eq!(fs::metadata(&key_path).unwrap().len(), 32);
}

#[test]
fn concurrent_first_start_returns_one_persisted_key() {
    let (_temporary, key_path) = isolated_key_path();
    let workers = 8;
    let barrier = Arc::new(Barrier::new(workers));
    let handles = (0..workers)
        .map(|_| {
            let barrier = barrier.clone();
            let key_path = key_path.clone();
            thread::spawn(move || {
                barrier.wait();
                load_or_create_master_key(key_path).unwrap()
            })
        })
        .collect::<Vec<_>>();
    let keys = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .collect::<Vec<_>>();

    for key in keys.iter().skip(1) {
        assert_same_key(&keys[0], key);
    }
    assert_eq!(fs::metadata(&key_path).unwrap().len(), 32);
}

#[test]
fn rejects_symlinked_credentials_directories_and_key_files() {
    let (_temporary, key_path) = isolated_key_path();
    let data_dir = key_path.parent().unwrap().parent().unwrap();
    let real_credentials = data_dir.join("real-credentials");
    fs::create_dir(&real_credentials).unwrap();
    fs::set_permissions(&real_credentials, fs::Permissions::from_mode(0o700)).unwrap();
    symlink(&real_credentials, data_dir.join("credentials")).unwrap();

    assert!(matches!(
        load_or_create_master_key(&key_path),
        Err(CredentialKeyStoreError::UnsafeCredentialsDirectory)
    ));

    fs::remove_file(data_dir.join("credentials")).unwrap();
    fs::create_dir(key_path.parent().unwrap()).unwrap();
    fs::set_permissions(
        key_path.parent().unwrap(),
        fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    let target = data_dir.join("target.key");
    fs::write(&target, [0x41; 32]).unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
    symlink(&target, &key_path).unwrap();

    assert!(matches!(
        load_or_create_master_key(&key_path),
        Err(CredentialKeyStoreError::UnsafeMasterKeyFile)
    ));
}

#[test]
fn rejects_non_regular_files_wrong_lengths_and_unsafe_permissions() {
    let (_temporary, key_path) = isolated_key_path();
    let credentials_dir = key_path.parent().unwrap();
    fs::create_dir(credentials_dir).unwrap();
    fs::set_permissions(credentials_dir, fs::Permissions::from_mode(0o700)).unwrap();
    fs::create_dir(&key_path).unwrap();

    assert!(matches!(
        load_or_create_master_key(&key_path),
        Err(CredentialKeyStoreError::UnsafeMasterKeyFile)
    ));

    fs::remove_dir(&key_path).unwrap();
    fs::write(&key_path, [0x42; 31]).unwrap();
    fs::set_permissions(&key_path, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(matches!(
        load_or_create_master_key(&key_path),
        Err(CredentialKeyStoreError::InvalidMasterKeyLength)
    ));

    fs::write(&key_path, [0x43; 32]).unwrap();
    fs::set_permissions(&key_path, fs::Permissions::from_mode(0o640)).unwrap();
    assert!(matches!(
        load_or_create_master_key(&key_path),
        Err(CredentialKeyStoreError::UnsafeMasterKeyPermissions)
    ));

    fs::set_permissions(&key_path, fs::Permissions::from_mode(0o600)).unwrap();
    fs::set_permissions(credentials_dir, fs::Permissions::from_mode(0o750)).unwrap();
    assert!(matches!(
        load_or_create_master_key(&key_path),
        Err(CredentialKeyStoreError::UnsafeCredentialsDirectoryPermissions)
    ));
}

#[test]
fn rejects_special_permission_bits_and_additional_hard_links() {
    let (_temporary, key_path) = isolated_key_path();
    let credentials_dir = key_path.parent().unwrap();
    fs::create_dir(credentials_dir).unwrap();
    fs::set_permissions(credentials_dir, fs::Permissions::from_mode(0o1700)).unwrap();

    assert!(matches!(
        load_or_create_master_key(&key_path),
        Err(CredentialKeyStoreError::UnsafeCredentialsDirectoryPermissions)
    ));

    fs::set_permissions(credentials_dir, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(&key_path, [0x45; 32]).unwrap();
    fs::set_permissions(&key_path, fs::Permissions::from_mode(0o2600)).unwrap();
    assert!(matches!(
        load_or_create_master_key(&key_path),
        Err(CredentialKeyStoreError::UnsafeMasterKeyPermissions)
    ));

    fs::set_permissions(&key_path, fs::Permissions::from_mode(0o600)).unwrap();
    fs::hard_link(&key_path, credentials_dir.join("copied.key")).unwrap();
    assert!(matches!(
        load_or_create_master_key(&key_path),
        Err(CredentialKeyStoreError::UnsafeMasterKeyFile)
    ));
}

#[test]
fn rejects_credentials_and_keys_owned_by_another_user() {
    if !geteuid().is_root() {
        return;
    }

    let (_temporary, key_path) = isolated_key_path();
    let credentials_dir = key_path.parent().unwrap();
    fs::create_dir(credentials_dir).unwrap();
    fs::set_permissions(credentials_dir, fs::Permissions::from_mode(0o700)).unwrap();
    chown(credentials_dir, Some(Uid::from_raw(1)), None).unwrap();
    assert!(matches!(
        load_or_create_master_key(&key_path),
        Err(CredentialKeyStoreError::UnsafeCredentialsDirectoryOwner)
    ));

    chown(credentials_dir, Some(Uid::ROOT), None).unwrap();
    fs::write(&key_path, [0x46; 32]).unwrap();
    fs::set_permissions(&key_path, fs::Permissions::from_mode(0o600)).unwrap();
    chown(&key_path, Some(Uid::from_raw(1)), None).unwrap();
    assert!(matches!(
        load_or_create_master_key(&key_path),
        Err(CredentialKeyStoreError::UnsafeMasterKeyOwner)
    ));
}

#[test]
fn removes_a_crash_left_pending_key_before_generating_the_master_key() {
    let (_temporary, key_path) = isolated_key_path();
    let credentials_dir = key_path.parent().unwrap();
    fs::create_dir(credentials_dir).unwrap();
    fs::set_permissions(credentials_dir, fs::Permissions::from_mode(0o700)).unwrap();
    let pending_path = credentials_dir.join(".master.key.pending");
    fs::write(&pending_path, [0x47; 32]).unwrap();
    fs::set_permissions(&pending_path, fs::Permissions::from_mode(0o600)).unwrap();

    load_or_create_master_key(&key_path).unwrap();

    assert!(!pending_path.exists());
    assert_eq!(fs::metadata(&key_path).unwrap().len(), 32);
    assert_ne!(fs::read(&key_path).unwrap(), vec![0x47; 32]);
}

#[cfg(target_os = "macos")]
#[test]
fn rejects_extended_acls_on_credentials_directories_and_keys() {
    let (_temporary, key_path) = isolated_key_path();
    let credentials_dir = key_path.parent().unwrap();
    fs::create_dir(credentials_dir).unwrap();
    fs::set_permissions(credentials_dir, fs::Permissions::from_mode(0o700)).unwrap();
    setfacl(
        &[credentials_dir],
        &[AclEntry::allow_group(
            "everyone",
            Perm::READ | Perm::EXECUTE,
            None,
        )],
        None,
    )
    .unwrap();
    let error = load_or_create_master_key(&key_path).unwrap_err();
    assert!(
        matches!(
            error,
            CredentialKeyStoreError::UnsafeCredentialsDirectoryAcl
        ),
        "{error:?}"
    );

    setfacl(&[credentials_dir], &[], None).unwrap();
    fs::write(&key_path, [0x48; 32]).unwrap();
    fs::set_permissions(&key_path, fs::Permissions::from_mode(0o600)).unwrap();
    setfacl(
        &[&key_path],
        &[AclEntry::allow_group("everyone", Perm::READ, None)],
        None,
    )
    .unwrap();
    let error = load_or_create_master_key(&key_path).unwrap_err();
    assert!(
        matches!(error, CredentialKeyStoreError::UnsafeMasterKeyAcl),
        "{error:?}"
    );
}

#[test]
fn debug_and_errors_never_expose_master_key_bytes() {
    let (_temporary, key_path) = isolated_key_path();
    let credentials_dir = key_path.parent().unwrap();
    fs::create_dir(credentials_dir).unwrap();
    fs::set_permissions(credentials_dir, fs::Permissions::from_mode(0o700)).unwrap();
    let canary = b"master-key-debug-canary";
    let mut stored = [0x44; 32];
    stored[..canary.len()].copy_from_slice(canary);
    fs::write(&key_path, stored).unwrap();
    fs::set_permissions(&key_path, fs::Permissions::from_mode(0o600)).unwrap();

    let key = load_or_create_master_key(&key_path).unwrap();
    let rendered = format!("{key:?}");
    assert!(!rendered.contains("master-key-debug-canary"));
    assert!(rendered.contains("[REDACTED]"));

    fs::write(&key_path, canary).unwrap();
    let error = load_or_create_master_key(&key_path).unwrap_err();
    let rendered = format!("{error:?}\n{error}");
    assert!(!rendered.contains("master-key-debug-canary"));
}
