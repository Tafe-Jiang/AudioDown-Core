use std::{
    collections::HashSet,
    fs::{File, OpenOptions},
    io,
    path::{Component, Path, PathBuf},
};

use flate2::read::GzDecoder;
use tar::Archive;

use crate::PluginManagerError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapshotLimits {
    pub max_compressed_bytes: u64,
    pub max_extracted_bytes: u64,
    pub max_file_bytes: u64,
    pub max_files: usize,
    pub max_plugins: usize,
}

impl Default for SnapshotLimits {
    fn default() -> Self {
        Self {
            max_compressed_bytes: 16 * 1024 * 1024,
            max_extracted_bytes: 64 * 1024 * 1024,
            max_file_bytes: 4 * 1024 * 1024,
            max_files: 2_048,
            max_plugins: 32,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractedSnapshot {
    pub repository_root: PathBuf,
    pub file_count: usize,
    pub extracted_bytes: u64,
}

pub async fn extract_snapshot(
    archive_path: &Path,
    destination: &Path,
    limits: SnapshotLimits,
) -> Result<ExtractedSnapshot, PluginManagerError> {
    let archive_path = archive_path.to_path_buf();
    let destination = destination.to_path_buf();
    tokio::task::spawn_blocking(move || {
        extract_snapshot_blocking(&archive_path, &destination, limits)
    })
    .await
    .map_err(|_| PluginManagerError::ArchiveIo)?
}

fn extract_snapshot_blocking(
    archive_path: &Path,
    destination: &Path,
    limits: SnapshotLimits,
) -> Result<ExtractedSnapshot, PluginManagerError> {
    let compressed_bytes = std::fs::metadata(archive_path)
        .map_err(|_| PluginManagerError::ArchiveIo)?
        .len();
    if compressed_bytes > limits.max_compressed_bytes {
        return Err(PluginManagerError::ArchiveCompressedTooLarge);
    }

    std::fs::create_dir_all(destination).map_err(|_| PluginManagerError::ArchiveIo)?;
    let temporary_root = destination.join("repository.tmp");
    let repository_root = destination.join("repository");
    if temporary_root.exists() {
        std::fs::remove_dir_all(&temporary_root).map_err(|_| PluginManagerError::ArchiveIo)?;
    }
    if repository_root.exists() {
        return Err(PluginManagerError::ArchiveIo);
    }
    create_directory(&temporary_root)?;

    let result = extract_entries(archive_path, &temporary_root, limits).and_then(
        |(file_count, extracted_bytes)| {
            std::fs::rename(&temporary_root, &repository_root)
                .map_err(|_| PluginManagerError::ArchiveIo)?;
            Ok(ExtractedSnapshot {
                repository_root: repository_root.clone(),
                file_count,
                extracted_bytes,
            })
        },
    );

    if result.is_err() {
        let _ = std::fs::remove_dir_all(&temporary_root);
    }
    result
}

fn extract_entries(
    archive_path: &Path,
    temporary_root: &Path,
    limits: SnapshotLimits,
) -> Result<(usize, u64), PluginManagerError> {
    let archive_file = File::open(archive_path).map_err(|_| PluginManagerError::ArchiveIo)?;
    let decoder = GzDecoder::new(archive_file);
    let mut archive = Archive::new(decoder);
    let entries = archive
        .entries()
        .map_err(|_| PluginManagerError::ArchiveInvalid)?;
    let mut common_root: Option<String> = None;
    let mut seen_paths = HashSet::new();
    let mut case_folded_paths = HashSet::new();
    let mut file_count = 0_usize;
    let mut extracted_bytes = 0_u64;

    for entry in entries {
        let mut entry = entry.map_err(|_| PluginManagerError::ArchiveInvalid)?;
        let entry_type = entry.header().entry_type();
        if !entry_type.is_file() && !entry_type.is_dir() {
            return Err(PluginManagerError::ArchiveUnsupportedEntry);
        }

        let path = entry
            .path()
            .map_err(|_| PluginManagerError::ArchiveInvalid)?;
        let components = normalize_path(&path)?;
        let root = &components[0];
        match &common_root {
            Some(expected) if expected != root => {
                return Err(PluginManagerError::ArchiveMultipleRoots);
            }
            None => common_root = Some(root.clone()),
            _ => {}
        }

        let relative = components[1..].iter().collect::<PathBuf>();
        if relative.as_os_str().is_empty() {
            if entry_type.is_dir() {
                continue;
            }
            return Err(PluginManagerError::ArchivePathEscape);
        }

        let normalized = relative
            .to_str()
            .ok_or(PluginManagerError::ArchiveNonUtf8Path)?
            .replace(std::path::MAIN_SEPARATOR, "/");
        if !seen_paths.insert(normalized.clone()) {
            return Err(PluginManagerError::ArchiveDuplicatePath);
        }
        if !case_folded_paths.insert(normalized.to_ascii_lowercase()) {
            return Err(PluginManagerError::ArchiveCaseConflict);
        }

        if entry_type.is_dir() {
            ensure_directory(temporary_root, &relative)?;
            continue;
        }

        file_count = file_count
            .checked_add(1)
            .ok_or(PluginManagerError::ArchiveTooManyFiles)?;
        if file_count > limits.max_files {
            return Err(PluginManagerError::ArchiveTooManyFiles);
        }
        let file_bytes = entry
            .header()
            .size()
            .map_err(|_| PluginManagerError::ArchiveInvalid)?;
        if file_bytes > limits.max_file_bytes {
            return Err(PluginManagerError::ArchiveFileTooLarge);
        }
        extracted_bytes = extracted_bytes
            .checked_add(file_bytes)
            .ok_or(PluginManagerError::ArchiveExtractedTooLarge)?;
        if extracted_bytes > limits.max_extracted_bytes {
            return Err(PluginManagerError::ArchiveExtractedTooLarge);
        }

        let parent = relative.parent().unwrap_or_else(|| Path::new(""));
        ensure_directory(temporary_root, parent)?;
        let output_path = temporary_root.join(&relative);
        let mut output = secure_file(&output_path)?;
        let copied =
            io::copy(&mut entry, &mut output).map_err(|_| PluginManagerError::ArchiveIo)?;
        if copied != file_bytes {
            return Err(PluginManagerError::ArchiveInvalid);
        }
        output
            .sync_all()
            .map_err(|_| PluginManagerError::ArchiveIo)?;
    }

    if common_root.is_none() {
        return Err(PluginManagerError::ArchiveInvalid);
    }
    Ok((file_count, extracted_bytes))
}

fn normalize_path(path: &Path) -> Result<Vec<String>, PluginManagerError> {
    let path = path
        .to_str()
        .ok_or(PluginManagerError::ArchiveNonUtf8Path)?;
    if path.contains('\\') {
        return Err(PluginManagerError::ArchivePathEscape);
    }

    let mut normalized = Vec::new();
    for component in Path::new(path).components() {
        match component {
            Component::Normal(value) => normalized.push(
                value
                    .to_str()
                    .ok_or(PluginManagerError::ArchiveNonUtf8Path)?
                    .to_string(),
            ),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(PluginManagerError::ArchivePathEscape);
            }
        }
    }
    if normalized.is_empty() {
        return Err(PluginManagerError::ArchivePathEscape);
    }
    Ok(normalized)
}

fn ensure_directory(root: &Path, relative: &Path) -> Result<(), PluginManagerError> {
    let mut current = root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(component) = component else {
            return Err(PluginManagerError::ArchivePathEscape);
        };
        current.push(component);
        match create_directory(&current) {
            Ok(()) => {}
            Err(PluginManagerError::ArchiveIo) if current.is_dir() => {
                set_directory_mode(&current)?;
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn create_directory(path: &Path) -> Result<(), PluginManagerError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;

        let mut builder = std::fs::DirBuilder::new();
        builder.mode(0o700);
        builder
            .create(path)
            .map_err(|_| PluginManagerError::ArchiveIo)?;
        set_directory_mode(path)
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir(path).map_err(|_| PluginManagerError::ArchiveIo)
    }
}

fn secure_file(path: &Path) -> Result<File, PluginManagerError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .map_err(|_| PluginManagerError::ArchiveIo)
    }
    #[cfg(not(unix))]
    {
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|_| PluginManagerError::ArchiveIo)
    }
}

#[cfg(unix)]
fn set_directory_mode(path: &Path) -> Result<(), PluginManagerError> {
    use std::os::unix::fs::PermissionsExt;

    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
        .map_err(|_| PluginManagerError::ArchiveIo)
}

#[cfg(not(unix))]
fn set_directory_mode(_path: &Path) -> Result<(), PluginManagerError> {
    Ok(())
}
