use std::{
    ffi::OsStr,
    fs::File,
    io::{self, Read, Write},
    os::fd::{AsFd, OwnedFd},
    path::Path,
    sync::Mutex,
};

use rand_core::{OsRng, RngCore};
use rustix::{
    fs::{self, AtFlags, FileType, FlockOperation, Mode, OFlags, Stat},
    io::Errno,
    process::geteuid,
};
use secrecy::Secret;
use thiserror::Error;
use zeroize::Zeroizing;

use crate::MasterKey;

const MASTER_KEY_BYTES: usize = 32;
const CREDENTIALS_DIRECTORY_MODE: u32 = 0o700;
const MASTER_KEY_FILE_MODE: u32 = 0o600;
const BOOTSTRAP_LOCK_NAME: &str = ".master.key.lock";
const PENDING_KEY_NAME: &str = ".master.key.pending";

static PROCESS_BOOTSTRAP_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Error)]
pub enum CredentialKeyStoreError {
    #[error("credential key path must have a credentials directory")]
    MissingCredentialsDirectory,
    #[error("credential directory is not a safe regular directory")]
    UnsafeCredentialsDirectory,
    #[error("credential directory owner is unsafe")]
    UnsafeCredentialsDirectoryOwner,
    #[error("credential directory access control list is unsafe")]
    UnsafeCredentialsDirectoryAcl,
    #[error("credential directory permissions are unsafe")]
    UnsafeCredentialsDirectoryPermissions,
    #[error("credential bootstrap lock is unsafe")]
    UnsafeBootstrapLock,
    #[error("credential pending key file is unsafe")]
    UnsafePendingKeyFile,
    #[error("credential master key is not a safe regular file")]
    UnsafeMasterKeyFile,
    #[error("credential master key owner is unsafe")]
    UnsafeMasterKeyOwner,
    #[error("credential master key access control list is unsafe")]
    UnsafeMasterKeyAcl,
    #[error("credential master key permissions are unsafe")]
    UnsafeMasterKeyPermissions,
    #[error("credential master key must contain exactly 32 bytes")]
    InvalidMasterKeyLength,
    #[error("operating system randomness is unavailable")]
    RandomnessUnavailable,
    #[error("credential key bootstrap lock is unavailable")]
    BootstrapLockUnavailable,
    #[error("credential key store I/O failed")]
    Io(#[source] io::Error),
}

pub fn load_or_create_master_key(
    path: impl AsRef<Path>,
) -> Result<MasterKey, CredentialKeyStoreError> {
    let _process_guard = PROCESS_BOOTSTRAP_LOCK
        .lock()
        .map_err(|_| CredentialKeyStoreError::BootstrapLockUnavailable)?;
    let location = KeyLocation::parse(path.as_ref())?;
    let credentials = open_credentials_directory(&location)?;
    let _bootstrap_lock = acquire_bootstrap_lock(&credentials)?;
    cleanup_pending_key(&credentials)?;

    match fs::statat(&credentials, location.key_name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(_) => {
            sync_directory(&credentials)?;
            load_master_key(&credentials, location.key_name)
        }
        Err(Errno::NOENT) => create_master_key(&credentials, location.key_name),
        Err(error) => Err(io_error(error)),
    }
}

struct KeyLocation<'a> {
    data_dir: &'a Path,
    credentials_name: &'a OsStr,
    key_name: &'a OsStr,
}

impl<'a> KeyLocation<'a> {
    fn parse(path: &'a Path) -> Result<Self, CredentialKeyStoreError> {
        let credentials_dir = path
            .parent()
            .ok_or(CredentialKeyStoreError::MissingCredentialsDirectory)?;
        let data_dir = credentials_dir
            .parent()
            .ok_or(CredentialKeyStoreError::MissingCredentialsDirectory)?;
        let credentials_name = credentials_dir
            .file_name()
            .ok_or(CredentialKeyStoreError::MissingCredentialsDirectory)?;
        let key_name = path
            .file_name()
            .ok_or(CredentialKeyStoreError::MissingCredentialsDirectory)?;
        Ok(Self {
            data_dir,
            credentials_name,
            key_name,
        })
    }
}

fn open_credentials_directory(
    location: &KeyLocation<'_>,
) -> Result<OwnedFd, CredentialKeyStoreError> {
    let data_dir =
        fs::open(location.data_dir, directory_open_flags(), Mode::empty()).map_err(io_error)?;
    let created = match fs::mkdirat(&data_dir, location.credentials_name, Mode::RWXU) {
        Ok(()) => true,
        Err(Errno::EXIST) => false,
        Err(error) => return Err(io_error(error)),
    };
    let credentials = fs::openat(
        &data_dir,
        location.credentials_name,
        directory_open_flags(),
        Mode::empty(),
    )
    .map_err(|_| CredentialKeyStoreError::UnsafeCredentialsDirectory)?;
    if created {
        fs::fchmod(&credentials, Mode::RWXU).map_err(io_error)?;
        clear_extended_acl(&credentials)?;
    }
    validate_credentials_directory(&fs::fstat(&credentials).map_err(io_error)?)?;
    validate_no_extended_acl(
        &credentials,
        CredentialKeyStoreError::UnsafeCredentialsDirectoryAcl,
    )?;
    if created {
        sync_directory(&credentials)?;
        sync_directory(&data_dir)?;
    }
    Ok(credentials)
}

fn acquire_bootstrap_lock(credentials: &OwnedFd) -> Result<OwnedFd, CredentialKeyStoreError> {
    let lock = match fs::openat(
        credentials,
        OsStr::new(BOOTSTRAP_LOCK_NAME),
        OFlags::CREATE | OFlags::EXCL | OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::RUSR | Mode::WUSR,
    ) {
        Ok(lock) => {
            fs::fchmod(&lock, Mode::RUSR | Mode::WUSR).map_err(io_error)?;
            clear_extended_acl(&lock)?;
            fs::fsync(&lock).map_err(io_error)?;
            sync_directory(credentials)?;
            lock
        }
        Err(Errno::EXIST) => fs::openat(
            credentials,
            BOOTSTRAP_LOCK_NAME,
            OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::empty(),
        )
        .map_err(|_| CredentialKeyStoreError::UnsafeBootstrapLock)?,
        Err(error) => return Err(io_error(error)),
    };

    validate_auxiliary_file(
        &fs::fstat(&lock).map_err(io_error)?,
        CredentialKeyStoreError::UnsafeBootstrapLock,
        true,
    )?;
    validate_no_extended_acl(&lock, CredentialKeyStoreError::UnsafeBootstrapLock)?;
    fs::flock(&lock, FlockOperation::LockExclusive).map_err(io_error)?;
    validate_auxiliary_file(
        &fs::fstat(&lock).map_err(io_error)?,
        CredentialKeyStoreError::UnsafeBootstrapLock,
        true,
    )?;
    validate_no_extended_acl(&lock, CredentialKeyStoreError::UnsafeBootstrapLock)?;
    validate_open_entry(
        credentials,
        OsStr::new(BOOTSTRAP_LOCK_NAME),
        &lock,
        CredentialKeyStoreError::UnsafeBootstrapLock,
    )?;
    Ok(lock)
}

fn cleanup_pending_key(credentials: &OwnedFd) -> Result<(), CredentialKeyStoreError> {
    match fs::statat(credentials, PENDING_KEY_NAME, AtFlags::SYMLINK_NOFOLLOW) {
        Err(Errno::NOENT) => Ok(()),
        Err(error) => Err(io_error(error)),
        Ok(stat) => {
            validate_auxiliary_file(&stat, CredentialKeyStoreError::UnsafePendingKeyFile, false)?;
            fs::unlinkat(credentials, PENDING_KEY_NAME, AtFlags::empty()).map_err(io_error)?;
            sync_directory(credentials)
        }
    }
}

fn create_master_key(
    credentials: &OwnedFd,
    key_name: &OsStr,
) -> Result<MasterKey, CredentialKeyStoreError> {
    let mut key_bytes = Zeroizing::new([0_u8; MASTER_KEY_BYTES]);
    OsRng
        .try_fill_bytes(key_bytes.as_mut())
        .map_err(|_| CredentialKeyStoreError::RandomnessUnavailable)?;

    let pending = fs::openat(
        credentials,
        PENDING_KEY_NAME,
        OFlags::CREATE | OFlags::EXCL | OFlags::WRONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::RUSR | Mode::WUSR,
    )
    .map_err(io_error)?;
    let pending_file = match prepare_pending_key(credentials, pending, &key_bytes) {
        Ok(file) => file,
        Err(error) => {
            cleanup_created_pending(credentials)?;
            return Err(error);
        }
    };

    match fs::linkat(
        credentials,
        PENDING_KEY_NAME,
        credentials,
        key_name,
        AtFlags::empty(),
    ) {
        Ok(()) => {}
        Err(Errno::EXIST) => {
            cleanup_created_pending(credentials)?;
            sync_directory(credentials)?;
            return load_master_key(credentials, key_name);
        }
        Err(error) => {
            cleanup_created_pending(credentials)?;
            return Err(io_error(error));
        }
    }

    fs::unlinkat(credentials, PENDING_KEY_NAME, AtFlags::empty()).map_err(io_error)?;
    sync_directory(credentials)?;
    validate_published_key(credentials, key_name, &pending_file)?;
    Ok(MasterKey::from_secret(Secret::new(*key_bytes)))
}

fn prepare_pending_key(
    credentials: &OwnedFd,
    pending: OwnedFd,
    key_bytes: &Zeroizing<[u8; MASTER_KEY_BYTES]>,
) -> Result<File, CredentialKeyStoreError> {
    fs::fchmod(&pending, Mode::RUSR | Mode::WUSR).map_err(io_error)?;
    clear_extended_acl(&pending)?;
    validate_auxiliary_file(
        &fs::fstat(&pending).map_err(io_error)?,
        CredentialKeyStoreError::UnsafePendingKeyFile,
        true,
    )?;
    validate_no_extended_acl(&pending, CredentialKeyStoreError::UnsafePendingKeyFile)?;
    let mut file = File::from(pending);
    file.write_all(key_bytes.as_ref())
        .map_err(CredentialKeyStoreError::Io)?;
    file.sync_all().map_err(CredentialKeyStoreError::Io)?;
    sync_directory(credentials)?;
    Ok(file)
}

fn cleanup_created_pending(credentials: &OwnedFd) -> Result<(), CredentialKeyStoreError> {
    match fs::unlinkat(credentials, PENDING_KEY_NAME, AtFlags::empty()) {
        Ok(()) | Err(Errno::NOENT) => sync_directory(credentials),
        Err(error) => Err(io_error(error)),
    }
}

fn load_master_key(
    credentials: &OwnedFd,
    key_name: &OsStr,
) -> Result<MasterKey, CredentialKeyStoreError> {
    let key = fs::openat(
        credentials,
        key_name,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .map_err(|_| CredentialKeyStoreError::UnsafeMasterKeyFile)?;
    validate_master_key_metadata(&fs::fstat(&key).map_err(io_error)?)?;
    validate_no_extended_acl(&key, CredentialKeyStoreError::UnsafeMasterKeyAcl)?;
    validate_open_entry(
        credentials,
        key_name,
        &key,
        CredentialKeyStoreError::UnsafeMasterKeyFile,
    )?;

    let mut file = File::from(key);
    let mut key_bytes = Zeroizing::new([0_u8; MASTER_KEY_BYTES]);
    if let Err(error) = file.read_exact(key_bytes.as_mut()) {
        return if error.kind() == io::ErrorKind::UnexpectedEof {
            Err(CredentialKeyStoreError::InvalidMasterKeyLength)
        } else {
            Err(CredentialKeyStoreError::Io(error))
        };
    }
    let mut trailing = [0_u8; 1];
    match file.read(&mut trailing) {
        Ok(0) => Ok(MasterKey::from_secret(Secret::new(*key_bytes))),
        Ok(_) => Err(CredentialKeyStoreError::InvalidMasterKeyLength),
        Err(error) => Err(CredentialKeyStoreError::Io(error)),
    }
}

fn validate_published_key(
    credentials: &OwnedFd,
    key_name: &OsStr,
    pending_file: &File,
) -> Result<(), CredentialKeyStoreError> {
    let published = fs::statat(credentials, key_name, AtFlags::SYMLINK_NOFOLLOW)
        .map_err(|_| CredentialKeyStoreError::UnsafeMasterKeyFile)?;
    let opened = fs::fstat(pending_file).map_err(io_error)?;
    validate_master_key_metadata(&published)?;
    validate_no_extended_acl(pending_file, CredentialKeyStoreError::UnsafeMasterKeyAcl)?;
    if published.st_dev != opened.st_dev || published.st_ino != opened.st_ino {
        return Err(CredentialKeyStoreError::UnsafeMasterKeyFile);
    }
    Ok(())
}

fn validate_open_entry<Fd: AsFd>(
    credentials: &OwnedFd,
    name: &OsStr,
    opened: Fd,
    error: CredentialKeyStoreError,
) -> Result<(), CredentialKeyStoreError> {
    let path_stat = fs::statat(credentials, name, AtFlags::SYMLINK_NOFOLLOW)
        .map_err(|_| clone_security_error(&error))?;
    let opened_stat = fs::fstat(opened).map_err(io_error)?;
    if path_stat.st_dev != opened_stat.st_dev || path_stat.st_ino != opened_stat.st_ino {
        return Err(error);
    }
    Ok(())
}

fn validate_credentials_directory(stat: &Stat) -> Result<(), CredentialKeyStoreError> {
    if FileType::from_raw_mode(stat.st_mode) != FileType::Directory {
        return Err(CredentialKeyStoreError::UnsafeCredentialsDirectory);
    }
    if stat.st_uid != geteuid().as_raw() {
        return Err(CredentialKeyStoreError::UnsafeCredentialsDirectoryOwner);
    }
    if exact_mode(stat) != CREDENTIALS_DIRECTORY_MODE {
        return Err(CredentialKeyStoreError::UnsafeCredentialsDirectoryPermissions);
    }
    Ok(())
}

fn validate_master_key_metadata(stat: &Stat) -> Result<(), CredentialKeyStoreError> {
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile || stat.st_nlink != 1 {
        return Err(CredentialKeyStoreError::UnsafeMasterKeyFile);
    }
    if stat.st_uid != geteuid().as_raw() {
        return Err(CredentialKeyStoreError::UnsafeMasterKeyOwner);
    }
    if exact_mode(stat) != MASTER_KEY_FILE_MODE {
        return Err(CredentialKeyStoreError::UnsafeMasterKeyPermissions);
    }
    Ok(())
}

fn validate_auxiliary_file(
    stat: &Stat,
    error: CredentialKeyStoreError,
    require_single_link: bool,
) -> Result<(), CredentialKeyStoreError> {
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile
        || require_single_link && stat.st_nlink != 1
        || stat.st_uid != geteuid().as_raw()
        || exact_mode(stat) != MASTER_KEY_FILE_MODE
    {
        return Err(error);
    }
    Ok(())
}

fn exact_mode(stat: &Stat) -> u32 {
    Mode::from_raw_mode(stat.st_mode).as_raw_mode().into()
}

fn directory_open_flags() -> OFlags {
    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW
}

fn sync_directory<Fd: AsFd>(directory: Fd) -> Result<(), CredentialKeyStoreError> {
    fs::fsync(directory).map_err(io_error)
}

#[cfg(target_os = "macos")]
fn clear_extended_acl<Fd: AsFd>(file: Fd) -> Result<(), CredentialKeyStoreError> {
    use std::os::fd::AsRawFd;

    macos_acl::clear(file.as_fd().as_raw_fd()).map_err(CredentialKeyStoreError::Io)
}

#[cfg(not(target_os = "macos"))]
fn clear_extended_acl<Fd: AsFd>(_file: Fd) -> Result<(), CredentialKeyStoreError> {
    Ok(())
}

#[cfg(target_os = "macos")]
fn validate_no_extended_acl<Fd: AsFd>(
    file: Fd,
    error: CredentialKeyStoreError,
) -> Result<(), CredentialKeyStoreError> {
    use std::os::fd::AsRawFd;

    if macos_acl::has_entries(file.as_fd().as_raw_fd()).map_err(CredentialKeyStoreError::Io)? {
        Err(error)
    } else {
        Ok(())
    }
}

#[cfg(not(target_os = "macos"))]
fn validate_no_extended_acl<Fd: AsFd>(
    _file: Fd,
    _error: CredentialKeyStoreError,
) -> Result<(), CredentialKeyStoreError> {
    Ok(())
}

fn clone_security_error(error: &CredentialKeyStoreError) -> CredentialKeyStoreError {
    match error {
        CredentialKeyStoreError::UnsafeBootstrapLock => {
            CredentialKeyStoreError::UnsafeBootstrapLock
        }
        CredentialKeyStoreError::UnsafeMasterKeyFile => {
            CredentialKeyStoreError::UnsafeMasterKeyFile
        }
        _ => CredentialKeyStoreError::UnsafeMasterKeyFile,
    }
}

fn io_error(error: Errno) -> CredentialKeyStoreError {
    CredentialKeyStoreError::Io(io::Error::from_raw_os_error(error.raw_os_error()))
}

#[cfg(target_os = "macos")]
mod macos_acl {
    use std::{
        ffi::{c_int, c_uint, c_void},
        io,
        os::fd::RawFd,
        ptr::{self, NonNull},
    };

    const ACL_TYPE_EXTENDED: c_uint = 0x0000_0100;
    const ACL_FIRST_ENTRY: c_int = 0;

    unsafe extern "C" {
        fn acl_free(object: *mut c_void) -> c_int;
        fn acl_get_entry(acl: *mut c_void, entry_id: c_int, entry: *mut *mut c_void) -> c_int;
        fn acl_get_fd_np(fd: c_int, acl_type: c_uint) -> *mut c_void;
        fn acl_init(count: c_int) -> *mut c_void;
        fn acl_set_fd_np(fd: c_int, acl: *mut c_void, acl_type: c_uint) -> c_int;
    }

    struct OwnedAcl(NonNull<c_void>);

    impl OwnedAcl {
        fn initialize_empty() -> io::Result<Self> {
            // SAFETY: acl_init has no pointer arguments and returns an owned ACL object.
            let acl = unsafe { acl_init(1) };
            NonNull::new(acl)
                .map(Self)
                .ok_or_else(io::Error::last_os_error)
        }

        fn read_from_fd(fd: RawFd) -> io::Result<Option<Self>> {
            // SAFETY: fd is borrowed from an open Rust-owned file descriptor for this call.
            let acl = unsafe { acl_get_fd_np(fd, ACL_TYPE_EXTENDED) };
            match NonNull::new(acl) {
                Some(acl) => Ok(Some(Self(acl))),
                None => {
                    let error = io::Error::last_os_error();
                    if error.raw_os_error() == Some(libc_enoent()) {
                        Ok(None)
                    } else {
                        Err(error)
                    }
                }
            }
        }

        fn as_ptr(&self) -> *mut c_void {
            self.0.as_ptr()
        }
    }

    impl Drop for OwnedAcl {
        fn drop(&mut self) {
            // SAFETY: this pointer came from acl_init or acl_get_fd_np and is freed once here.
            let result = unsafe { acl_free(self.as_ptr()) };
            debug_assert_eq!(result, 0);
        }
    }

    pub(super) fn clear(fd: RawFd) -> io::Result<()> {
        let acl = OwnedAcl::initialize_empty()?;
        // SAFETY: fd remains open for the call and acl is a valid owned ACL object.
        let result = unsafe { acl_set_fd_np(fd, acl.as_ptr(), ACL_TYPE_EXTENDED) };
        if result == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    pub(super) fn has_entries(fd: RawFd) -> io::Result<bool> {
        let Some(acl) = OwnedAcl::read_from_fd(fd)? else {
            return Ok(false);
        };
        let mut entry = ptr::null_mut();
        // SAFETY: acl is valid and entry points to writable storage for one borrowed entry.
        let result = unsafe { acl_get_entry(acl.as_ptr(), ACL_FIRST_ENTRY, &mut entry) };
        if result == 0 && !entry.is_null() {
            Ok(true)
        } else {
            Err(io::Error::last_os_error())
        }
    }

    const fn libc_enoent() -> c_int {
        2
    }
}
