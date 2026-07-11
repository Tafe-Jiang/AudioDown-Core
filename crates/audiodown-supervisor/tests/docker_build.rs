use std::{collections::BTreeMap, fs};

use audiodown_supervisor::{
    docker_build::{
        assembler_container_config, build_source_archive, builder_container_config,
        complete_tar_length, managed_image_plan, normalize_output_archive,
        operation_resource_names, proxy_container_config, trusted_image_inputs, ManagedImageInput,
    },
    trusted_images,
};
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use uuid::Uuid;

const OPERATION_ID: &str = "75de0d58-03f9-4db7-8a27-69ac7ddce8de";
const PLUGIN_ID: &str = "com.audiodown.virtual.content";
const COMMIT_SHA: &str = "0123456789abcdef0123456789abcdef01234567";
const SOURCE_HASH: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const MANIFEST_HASH: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";

#[test]
fn operation_resources_and_container_requests_are_fully_derived() {
    let operation_id = Uuid::parse_str(OPERATION_ID).unwrap();
    let names = operation_resource_names(operation_id);
    assert_eq!(
        names.internal_network,
        format!("audiodown-build-{OPERATION_ID}-internal")
    );
    assert_eq!(
        names.egress_network,
        format!("audiodown-build-{OPERATION_ID}-egress")
    );

    let builder = builder_container_config(operation_id, false);
    let builder_host = builder.host_config.unwrap();
    assert_eq!(
        builder.image.as_deref(),
        Some(trusted_images::BUILDER_IMAGE)
    );
    assert_eq!(
        builder.labels.as_ref().unwrap()["io.audiodown.managed"],
        "true"
    );
    assert_eq!(
        builder.labels.as_ref().unwrap()["io.audiodown.resource-role"],
        "plugin-build"
    );
    assert_eq!(
        builder.labels.as_ref().unwrap()["io.audiodown.operation-id"],
        OPERATION_ID
    );
    assert_eq!(builder.user.as_deref(), Some("10001:10001"));
    assert_eq!(
        builder.env.unwrap(),
        vec![
            "AUDIODOWN_ALLOW_LIFECYCLE_SCRIPTS=false",
            "HTTP_PROXY=http://audiodown-npm-proxy:18081",
            "HTTPS_PROXY=http://audiodown-npm-proxy:18081",
            "NODE_ENV=production",
            "NO_PROXY=",
        ]
    );
    assert_eq!(
        builder_host.network_mode,
        Some(names.internal_network.clone())
    );
    assert_eq!(builder_host.binds, Some(Vec::new()));
    assert_eq!(builder_host.devices, Some(Vec::new()));
    assert_eq!(builder_host.cap_drop, Some(vec!["ALL".to_string()]));
    assert_eq!(
        builder_host.security_opt,
        Some(vec!["no-new-privileges:true".to_string()])
    );
    assert_eq!(builder_host.readonly_rootfs, Some(true));
    assert_eq!(builder_host.memory, Some(512 * 1024 * 1024));
    assert_eq!(builder_host.memory_swap, Some(512 * 1024 * 1024));
    assert_eq!(builder_host.nano_cpus, Some(1_000_000_000));
    assert_eq!(builder_host.pids_limit, Some(128));
    assert_eq!(
        builder_host.tmpfs.unwrap()["/workspace"],
        "rw,nosuid,nodev,size=268435456,uid=10001,gid=10001,mode=0700"
    );

    let proxy = proxy_container_config(operation_id);
    assert_eq!(
        proxy.labels.as_ref().unwrap()["io.audiodown.managed"],
        "true"
    );
    assert_eq!(
        proxy.labels.as_ref().unwrap()["io.audiodown.resource-role"],
        "plugin-build-proxy"
    );
    assert_eq!(
        proxy.labels.as_ref().unwrap()["io.audiodown.operation-id"],
        OPERATION_ID
    );
    let proxy_host = proxy.host_config.unwrap();
    assert_eq!(
        proxy_host.network_mode,
        Some(names.internal_network.clone())
    );
    assert_eq!(proxy_host.binds, Some(Vec::new()));
    assert_eq!(proxy_host.devices, Some(Vec::new()));
    assert_eq!(proxy_host.readonly_rootfs, Some(true));
    assert_eq!(proxy_host.memory, Some(128 * 1024 * 1024));
    assert_eq!(proxy_host.memory_swap, Some(128 * 1024 * 1024));
    assert_eq!(proxy_host.nano_cpus, Some(500_000_000));
    assert_eq!(proxy_host.pids_limit, Some(64));

    let assembler = assembler_container_config(operation_id);
    assert_eq!(
        assembler.image.as_deref(),
        Some(trusted_images::RUNTIME_IMAGE)
    );
    assert_eq!(assembler.network_disabled, Some(true));
    assert_eq!(
        assembler.host_config.unwrap().network_mode.as_deref(),
        Some("none")
    );
}

#[test]
fn source_archive_is_normalized_and_input_ready_is_last() {
    let root = TempDir::new().unwrap();
    fs::create_dir(root.path().join("src")).unwrap();
    fs::write(root.path().join("src/index.js"), "module.exports = 1;\n").unwrap();
    fs::write(root.path().join("package.json"), "{}\n").unwrap();

    let archive = build_source_archive(root.path()).unwrap();
    let entries = parse_tar(&archive);
    assert_eq!(
        entries.keys().cloned().collect::<Vec<_>>(),
        vec![
            "input/".to_string(),
            "input/.input-ready".to_string(),
            "input/package.json".to_string(),
            "input/src/".to_string(),
            "input/src/index.js".to_string(),
        ]
    );
    assert_eq!(
        tar_paths(&archive).last().map(String::as_str),
        Some("input/.input-ready")
    );
    assert_eq!(entries["input/"].mode, 0o755);
    assert_eq!(entries["input/package.json"].mode, 0o644);
    assert_eq!(entries["input/package.json"].uid, 10001);
    assert_eq!(entries["input/package.json"].gid, 10001);
    assert_eq!(entries["input/.input-ready"].contents, b"ready\n");
}

#[cfg(unix)]
#[test]
fn source_archive_rejects_symlinks() {
    use std::os::unix::fs::symlink;

    let root = TempDir::new().unwrap();
    fs::write(root.path().join("target"), "target").unwrap();
    symlink(root.path().join("target"), root.path().join("link")).unwrap();
    assert_eq!(
        build_source_archive(root.path()).unwrap_err().code(),
        "UNSAFE_BUILD_INPUT"
    );
}

#[test]
fn downloaded_output_is_validated_and_repacked_without_metadata() {
    let untrusted = make_tar(vec![
        TarFixture::directory("output"),
        TarFixture::file("output/index.js", b"console.log('ok');\n", 0o777, 123, 456),
        TarFixture::symlink("output/current.js", "index.js"),
    ]);
    let normalized = normalize_output_archive(&untrusted).unwrap();
    let entries = parse_tar(&normalized);

    assert_eq!(
        entries.keys().cloned().collect::<Vec<_>>(),
        vec!["current.js".to_string(), "index.js".to_string()]
    );
    assert_eq!(entries["index.js"].mode, 0o644);
    assert_eq!(entries["index.js"].uid, 0);
    assert_eq!(entries["index.js"].gid, 0);
    assert_eq!(entries["index.js"].mtime, 0);
    assert_eq!(entries["current.js"].kind, b'2');
    assert_eq!(entries["current.js"].link_name, "index.js");
}

#[test]
fn downloaded_output_rejects_traversal_hardlinks_and_duplicates() {
    for archive in [
        make_tar(vec![TarFixture::file(
            "output/../escape",
            b"x",
            0o644,
            0,
            0,
        )]),
        make_tar(vec![TarFixture::hardlink("output/hard", "output/index.js")]),
        make_tar(vec![
            TarFixture::file("output/index.js", b"a", 0o644, 0, 0),
            TarFixture::file("output/index.js", b"b", 0o644, 0, 0),
        ]),
        make_tar(vec![TarFixture::symlink("output/link", "../../etc/passwd")]),
    ] {
        assert_eq!(
            normalize_output_archive(&archive).unwrap_err().code(),
            "UNSAFE_BUILD_OUTPUT"
        );
    }
}

#[test]
fn streamed_docker_archives_stop_only_at_a_structural_tar_terminator() {
    let archive = make_tar(vec![
        TarFixture::file("output/zeros.bin", &vec![0; 1024], 0o644, 0, 0),
        TarFixture::file("output/index.js", b"module.exports = 1;\n", 0o644, 0, 0),
    ]);
    let complete = complete_tar_length(&archive).unwrap();
    assert!(complete <= archive.len());
    assert_eq!(complete_tar_length(&archive[..complete - 1]), None);
}

#[test]
fn trusted_inputs_and_managed_image_commit_are_deterministic() {
    let trusted = trusted_image_inputs().unwrap();
    assert!(trusted
        .pinned_base_reference
        .starts_with("node:22-bookworm-slim@sha256:"));
    assert_eq!(trusted.base_image_digest.len(), 71);
    assert_eq!(trusted.sdk_hash.len(), 64);
    assert_eq!(trusted.builder_asset_hash.len(), 64);
    assert_eq!(trusted.runtime_asset_hash.len(), 64);
    assert_ne!(trusted.builder_asset_hash, trusted.runtime_asset_hash);
    assert!(String::from_utf8_lossy(&trusted.builder_context).contains("node22-build-runner.js"));
    let runtime_entries = parse_tar(&trusted.runtime_context);
    assert_eq!(
        runtime_entries["plugin-sdk/node/src/content.js"].contents,
        include_bytes!("../../../plugin-sdk/node/src/content.js")
    );
    assert!(runtime_entries.contains_key("plugin-sdk/node/src/index.js"));
    assert_eq!(trusted.sdk_hash, complete_embedded_sdk_hash());

    let operation_id = Uuid::parse_str(OPERATION_ID).unwrap();
    let plan = managed_image_plan(ManagedImageInput {
        installation_id: "installation-a",
        operation_id,
        plugin_id: PLUGIN_ID,
        commit_sha: COMMIT_SHA,
        source_hash: SOURCE_HASH,
        manifest_hash: MANIFEST_HASH,
        base_image_digest: &trusted.base_image_digest,
        sdk_hash: &trusted.sdk_hash,
    })
    .unwrap();
    assert!(plan.tag.starts_with("audiodown/plugin-"));
    assert_eq!(plan.labels["io.audiodown.operation-id"], OPERATION_ID);
    assert_eq!(plan.labels["io.audiodown.managed"], "true");
    assert_eq!(
        plan.labels["io.audiodown.base-image-digest"],
        trusted.base_image_digest
    );
    assert_eq!(plan.labels["io.audiodown.sdk-hash"], trusted.sdk_hash);
}

fn complete_embedded_sdk_hash() -> String {
    let files: [(&str, &[u8]); 8] = [
        (
            "package-lock.json",
            include_bytes!("../../../plugin-sdk/node/package-lock.json"),
        ),
        (
            "package.json",
            include_bytes!("../../../plugin-sdk/node/package.json"),
        ),
        (
            "src/content.js",
            include_bytes!("../../../plugin-sdk/node/src/content.js"),
        ),
        (
            "src/index.js",
            include_bytes!("../../../plugin-sdk/node/src/index.js"),
        ),
        (
            "src/logger.js",
            include_bytes!("../../../plugin-sdk/node/src/logger.js"),
        ),
        (
            "src/rpc.js",
            include_bytes!("../../../plugin-sdk/node/src/rpc.js"),
        ),
        (
            "test/content.test.js",
            include_bytes!("../../../plugin-sdk/node/test/content.test.js"),
        ),
        (
            "test/sdk.test.js",
            include_bytes!("../../../plugin-sdk/node/test/sdk.test.js"),
        ),
    ];
    let mut hasher = Sha256::new();
    for (relative, contents) in files {
        hasher.update((relative.len() as u64).to_be_bytes());
        hasher.update(relative.as_bytes());
        hasher.update((contents.len() as u64).to_be_bytes());
        hasher.update(contents);
    }
    format!("{:x}", hasher.finalize())
}

#[derive(Debug)]
struct ParsedTarEntry {
    mode: u64,
    uid: u64,
    gid: u64,
    mtime: u64,
    kind: u8,
    link_name: String,
    contents: Vec<u8>,
}

fn parse_tar(bytes: &[u8]) -> BTreeMap<String, ParsedTarEntry> {
    let mut entries = BTreeMap::new();
    let mut offset = 0;
    while offset + 512 <= bytes.len() {
        let header = &bytes[offset..offset + 512];
        if header.iter().all(|byte| *byte == 0) {
            break;
        }
        let name = text(&header[0..100]);
        let size = octal(&header[124..136]) as usize;
        let contents_start = offset + 512;
        let contents_end = contents_start + size;
        entries.insert(
            name,
            ParsedTarEntry {
                mode: octal(&header[100..108]),
                uid: octal(&header[108..116]),
                gid: octal(&header[116..124]),
                mtime: octal(&header[136..148]),
                kind: header[156],
                link_name: text(&header[157..257]),
                contents: bytes[contents_start..contents_end].to_vec(),
            },
        );
        offset = contents_start + size.div_ceil(512) * 512;
    }
    entries
}

fn tar_paths(bytes: &[u8]) -> Vec<String> {
    let mut paths = Vec::new();
    let mut offset = 0;
    while offset + 512 <= bytes.len() {
        let header = &bytes[offset..offset + 512];
        if header.iter().all(|byte| *byte == 0) {
            break;
        }
        paths.push(text(&header[0..100]));
        let size = octal(&header[124..136]) as usize;
        offset += 512 + size.div_ceil(512) * 512;
    }
    paths
}

fn text(field: &[u8]) -> String {
    let length = field
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(field.len());
    String::from_utf8(field[..length].to_vec()).unwrap()
}

fn octal(field: &[u8]) -> u64 {
    let text = String::from_utf8_lossy(field)
        .trim_matches(char::from(0))
        .trim()
        .to_string();
    if text.is_empty() {
        0
    } else {
        u64::from_str_radix(&text, 8).unwrap()
    }
}

struct TarFixture<'a> {
    path: &'a str,
    contents: &'a [u8],
    mode: u64,
    uid: u64,
    gid: u64,
    kind: u8,
    link_name: &'a str,
}

impl<'a> TarFixture<'a> {
    fn directory(path: &'a str) -> Self {
        Self {
            path,
            contents: b"",
            mode: 0o755,
            uid: 0,
            gid: 0,
            kind: b'5',
            link_name: "",
        }
    }

    fn file(path: &'a str, contents: &'a [u8], mode: u64, uid: u64, gid: u64) -> Self {
        Self {
            path,
            contents,
            mode,
            uid,
            gid,
            kind: b'0',
            link_name: "",
        }
    }

    fn symlink(path: &'a str, target: &'a str) -> Self {
        Self {
            path,
            contents: b"",
            mode: 0o777,
            uid: 0,
            gid: 0,
            kind: b'2',
            link_name: target,
        }
    }

    fn hardlink(path: &'a str, target: &'a str) -> Self {
        Self {
            path,
            contents: b"",
            mode: 0o644,
            uid: 0,
            gid: 0,
            kind: b'1',
            link_name: target,
        }
    }
}

fn make_tar(entries: Vec<TarFixture<'_>>) -> Vec<u8> {
    let mut archive = Vec::new();
    for entry in entries {
        let mut header = [0_u8; 512];
        write_text(&mut header[0..100], entry.path);
        write_octal(&mut header[100..108], entry.mode);
        write_octal(&mut header[108..116], entry.uid);
        write_octal(&mut header[116..124], entry.gid);
        write_octal(&mut header[124..136], entry.contents.len() as u64);
        write_octal(&mut header[136..148], 123);
        header[148..156].fill(b' ');
        header[156] = entry.kind;
        write_text(&mut header[157..257], entry.link_name);
        header[257..263].copy_from_slice(b"ustar\0");
        header[263..265].copy_from_slice(b"00");
        let checksum = header.iter().map(|byte| u64::from(*byte)).sum();
        write_checksum(&mut header[148..156], checksum);
        archive.extend_from_slice(&header);
        archive.extend_from_slice(entry.contents);
        archive.resize(archive.len().div_ceil(512) * 512, 0);
    }
    archive.resize(archive.len() + 1024, 0);
    archive
}

fn write_text(field: &mut [u8], value: &str) {
    field[..value.len()].copy_from_slice(value.as_bytes());
}

fn write_octal(field: &mut [u8], value: u64) {
    let value = format!("{:0width$o}\0", value, width = field.len() - 1);
    field.copy_from_slice(value.as_bytes());
}

fn write_checksum(field: &mut [u8], value: u64) {
    let value = format!("{value:06o}\0 ");
    field.copy_from_slice(value.as_bytes());
}
