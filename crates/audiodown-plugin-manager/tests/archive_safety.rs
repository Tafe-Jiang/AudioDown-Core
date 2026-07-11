use std::{
    fs::File,
    path::{Path, PathBuf},
};

use audiodown_plugin_manager::{
    archive::{extract_snapshot, SnapshotLimits},
    PluginManagerError,
};
use flate2::{write::GzEncoder, Compression};
use tar::{Builder, EntryType, Header};
use tempfile::TempDir;

const FOUR_MIB: usize = 4 * 1024 * 1024;

#[tokio::test]
async fn extracts_one_common_root_with_restricted_permissions() {
    let archive = build_archive(|builder| {
        append_entry(builder, b"root/", EntryType::Directory, None, &[]);
        append_entry(
            builder,
            b"root/audiodown-repository.json",
            EntryType::Regular,
            None,
            b"{}",
        );
    });
    let destination = TempDir::new().unwrap();

    let extracted = extract_snapshot(&archive.path, destination.path(), SnapshotLimits::default())
        .await
        .unwrap();

    assert_eq!(
        extracted.repository_root,
        destination.path().join("repository")
    );
    assert_eq!(extracted.file_count, 1);
    assert_eq!(extracted.extracted_bytes, 2);
    assert_eq!(
        std::fs::read(extracted.repository_root.join("audiodown-repository.json")).unwrap(),
        b"{}"
    );
    assert_mode(&extracted.repository_root, 0o700);
    assert_mode(
        &extracted.repository_root.join("audiodown-repository.json"),
        0o600,
    );
}

#[tokio::test]
async fn rejects_parent_and_absolute_paths() {
    for path in [b"../escape".as_slice(), b"/absolute".as_slice()] {
        assert_archive_error(
            build_archive(|builder| {
                append_entry(builder, path, EntryType::Regular, None, b"x");
            }),
            PluginManagerError::ArchivePathEscape,
        )
        .await;
    }
}

#[tokio::test]
async fn rejects_links_and_device_entries() {
    for (entry_type, target) in [
        (EntryType::Symlink, Some("../../outside")),
        (EntryType::Link, Some("root/target")),
        (EntryType::Char, None),
    ] {
        assert_archive_error(
            build_archive(|builder| {
                append_entry(builder, b"root/unsafe", entry_type, target, &[]);
            }),
            PluginManagerError::ArchiveUnsupportedEntry,
        )
        .await;
    }
}

#[tokio::test]
async fn rejects_multiple_top_level_directories() {
    assert_archive_error(
        build_archive(|builder| {
            append_entry(builder, b"root/a", EntryType::Regular, None, b"a");
            append_entry(builder, b"other/b", EntryType::Regular, None, b"b");
        }),
        PluginManagerError::ArchiveMultipleRoots,
    )
    .await;
}

#[tokio::test]
async fn rejects_duplicate_normalized_paths() {
    assert_archive_error(
        build_archive(|builder| {
            append_entry(builder, b"root/dir//file", EntryType::Regular, None, b"a");
            append_entry(builder, b"root/dir/file", EntryType::Regular, None, b"b");
        }),
        PluginManagerError::ArchiveDuplicatePath,
    )
    .await;
}

#[tokio::test]
async fn rejects_case_folded_duplicate_paths() {
    assert_archive_error(
        build_archive(|builder| {
            append_entry(builder, b"root/File", EntryType::Regular, None, b"a");
            append_entry(builder, b"root/file", EntryType::Regular, None, b"b");
        }),
        PluginManagerError::ArchiveCaseConflict,
    )
    .await;
}

#[tokio::test]
async fn rejects_non_utf8_paths() {
    assert_archive_error(
        build_archive(|builder| {
            append_entry(builder, b"root/\xff", EntryType::Regular, None, b"x");
        }),
        PluginManagerError::ArchiveNonUtf8Path,
    )
    .await;
}

#[tokio::test]
async fn rejects_more_than_two_thousand_forty_eight_files() {
    assert_archive_error(
        build_archive(|builder| {
            for index in 0..2_049 {
                append_entry(
                    builder,
                    format!("root/{index:04}").as_bytes(),
                    EntryType::Regular,
                    None,
                    &[],
                );
            }
        }),
        PluginManagerError::ArchiveTooManyFiles,
    )
    .await;
}

#[tokio::test]
async fn rejects_files_larger_than_four_mebibytes() {
    let content = vec![0_u8; FOUR_MIB + 1];
    assert_archive_error(
        build_archive(|builder| {
            append_entry(builder, b"root/large", EntryType::Regular, None, &content);
        }),
        PluginManagerError::ArchiveFileTooLarge,
    )
    .await;
}

#[tokio::test]
async fn rejects_more_than_sixty_four_mebibytes_extracted() {
    let full_file = vec![0_u8; FOUR_MIB];
    assert_archive_error(
        build_archive(|builder| {
            for index in 0..16 {
                append_entry(
                    builder,
                    format!("root/full-{index:02}").as_bytes(),
                    EntryType::Regular,
                    None,
                    &full_file,
                );
            }
            append_entry(builder, b"root/overflow", EntryType::Regular, None, b"x");
        }),
        PluginManagerError::ArchiveExtractedTooLarge,
    )
    .await;
}

async fn assert_archive_error(archive: ArchiveFixture, expected: PluginManagerError) {
    let destination = TempDir::new().unwrap();
    let error = extract_snapshot(&archive.path, destination.path(), SnapshotLimits::default())
        .await
        .unwrap_err();

    assert_eq!(
        std::mem::discriminant(&error),
        std::mem::discriminant(&expected)
    );
    assert!(!destination.path().join("repository").exists());
    assert!(!destination.path().join("repository.tmp").exists());
}

struct ArchiveFixture {
    _temp: TempDir,
    path: PathBuf,
}

fn build_archive(build: impl FnOnce(&mut Builder<GzEncoder<File>>)) -> ArchiveFixture {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("snapshot.tar.gz");
    let encoder = GzEncoder::new(File::create(&path).unwrap(), Compression::default());
    let mut builder = Builder::new(encoder);
    build(&mut builder);
    builder.into_inner().unwrap().finish().unwrap();
    ArchiveFixture { _temp: temp, path }
}

fn append_entry(
    builder: &mut Builder<GzEncoder<File>>,
    path: &[u8],
    entry_type: EntryType,
    link_name: Option<&str>,
    content: &[u8],
) {
    let mut header = Header::new_gnu();
    header.set_entry_type(entry_type);
    header.set_mode(0o777);
    header.set_uid(123);
    header.set_gid(456);
    header.set_mtime(1);
    header.set_size(content.len() as u64);
    if let Some(link_name) = link_name {
        header.set_link_name(link_name).unwrap();
    }
    let name = &mut header.as_mut_bytes()[..100];
    name.fill(0);
    name[..path.len()].copy_from_slice(path);
    header.set_cksum();
    builder.append(&header, content).unwrap();
}

#[cfg(unix)]
fn assert_mode(path: &Path, expected: u32) {
    use std::os::unix::fs::PermissionsExt;

    let mode = std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, expected);
}

#[cfg(not(unix))]
fn assert_mode(_path: &Path, _expected: u32) {}
